#!/usr/bin/env python3
"""Turn one read-only ``node_getStatus`` reply into a sustained-memory observation.

This is the RPC half of the per-node, per-tick contract documented in
``docs/operations/sustained-memory-qualification.md`` (#1452). The x-series
controller already reads ``node_getStatus`` from every host on each monitor
tick (``controller.py``); this module maps that same reply onto the
observation file that ``sample_sustained_memory.py`` joins with local
``/proc`` and cgroup evidence.

Height, tip hash and mining state are all taken from the *same* reply: the
node builds ``chainHeight``/``tipHash`` from one ``chain_state`` read and
``mintingActive`` from the same lock ``minting_getStatus.active`` reports, so
no non-atomic combination of separate RPC calls is needed.

It never launches, pauses or reconfigures a node, never sends transactions and
never synthesizes a missing field: anything malformed or absent is omitted and
described in ``errors`` so the sampler row (and later the analyzer) treats it
as incomplete evidence rather than a zero.

``totalTransactions`` is the cumulative chain-state transaction counter
(``ChainState::record_block`` adds every block's ``transactions.len()``), not
a count of the declared transfer workload. It is transfer evidence only for a
minting manifest (transfer minimum zero). A payments manifest refuses it: ``confirmed_transfers`` must
come from a reviewed transfer source (``--transfer-source``), otherwise the
field is omitted and the observation carries an error.
"""

import argparse
import http.client
import ipaddress
import json
import os
from pathlib import Path
import re
import ssl
import tempfile
import time
import urllib.parse
import urllib.request

from sustained_memory import validate_manifest

HEX64 = re.compile(r"[0-9a-f]{64}")
# A git commit id the adapter will compare: at least the 7-hex short form, so
# a one-character value can never "agree" by prefix (#1743).
COMMIT = re.compile(r"[0-9a-f]{7,64}")
# Explicit endpoint, no credentials, query strings or fragments (#1743):
#
# * ``http://`` only to a loopback or private-lab (RFC 1918 / ULA) address
#   literal, or ``localhost``. The node's JSON-RPC server answers POST on any
#   path, so a direct endpoint may use ``/`` or ``/rpc``.
# * ``https://`` to a named host, path exactly ``/rpc`` (the reverse-proxy
#   route the x-series controller uses, ``runtime.validate_rpc_url``), and
#   only with a pinned private CA (``--ca-file``).
#
# The runner ties this endpoint to the measured process before the
# experiment (runbook step 2).
RPC_URL = re.compile(
    r"(?P<scheme>https?)://(?P<host>[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?|\[[0-9a-f:.]+\])"
    r"(?::[1-9][0-9]{0,4})?(?P<path>/(?:rpc)?)"
)
RPC_URL_HELP = (
    "explicit endpoint: http://<loopback-or-private-address>[:port]/ or .../rpc, "
    "or https://<host>[:port]/rpc with --ca-file; no credentials, query or fragment"
)
MAX_REPLY_BYTES = 256 * 1024


PLAIN_HTTP_NETWORKS = tuple(
    ipaddress.ip_network(cidr)
    for cidr in (
        "127.0.0.0/8",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "::1/128",
        "fc00::/7",
    )
)


def plain_http_address_allowed(address):
    """Explicit loopback/RFC 1918/ULA allowlist (not ``is_private``, which
    is broader and varies across Python versions)."""
    if address.version == 6 and address.ipv4_mapped is not None:
        address = address.ipv4_mapped
    return any(
        address.version == network.version and address in network
        for network in PLAIN_HTTP_NETWORKS
    )


def rpc_url_problem(url, ca_file=None):
    """Return why ``url`` is not an acceptable RPC endpoint, or ``None``."""
    match = RPC_URL.fullmatch(url) if isinstance(url, str) else None
    if not match:
        return "explicit http(s)://host[:port]/ or /rpc URL required"
    try:
        port = urllib.parse.urlsplit(url).port
    except ValueError:
        return "invalid RPC port"
    if port is not None and not 1 <= port <= 65535:
        return "invalid RPC port"
    host = match["host"].strip("[]")
    if match["scheme"] == "http":
        if host == "localhost":
            return None
        try:
            address = ipaddress.ip_address(host)
        except ValueError:
            return "plain http requires a loopback or private-lab address literal"
        if not plain_http_address_allowed(address):
            return "plain http requires a loopback or private-lab address literal"
        return None
    if match["path"] != "/rpc":
        return "https endpoints must use the /rpc path"
    if ca_file is None:
        return "https endpoints require a pinned private CA (--ca-file)"
    return None


def is_count(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def observation_from_status(
    plan,
    node,
    status,
    observed_at_unix_s,
    *,
    rpc_errors=(),
    expected_network=None,
    transfer_source=None,
):
    """Map one ``node_getStatus`` result onto the sampler's observation JSON.

    ``pid``/``process_start_id``/``chain_id`` come from the immutable manifest
    node: the runner bound this endpoint to that process before t0, and the
    sampler re-verifies the live process identity on every row. The reply
    can only *contradict* identity (``network`` or ``gitCommit`` mismatch),
    never establish a new one.

    ``transfer_source`` is ``(confirmed_transfers, errors)`` from a reviewed
    transfer source (``read_transfer_source``). It is required for a payments
    manifest, whose ``confirmed_transfers`` may never come from
    ``totalTransactions``, and refused for a minting manifest.
    """
    observation = {
        "observed_at_unix_s": observed_at_unix_s,
        "pid": node["pid"],
        "process_start_id": node["process_start_id"],
        "chain_id": node["chain_id"],
        "errors": [str(error) for error in rpc_errors],
    }
    errors = observation["errors"]
    if not isinstance(status, dict):
        if not errors:
            errors.append("node_getStatus returned no result object")
        return observation
    height, tip = status.get("chainHeight"), status.get("tipHash")
    if is_count(height) and isinstance(tip, str) and HEX64.fullmatch(tip):
        observation["chain_height"] = height
        observation["chain_hash"] = tip
    else:
        errors.append("node_getStatus chainHeight/tipHash missing or malformed")
    if isinstance(status.get("mintingActive"), bool):
        observation["mining"] = status["mintingActive"]
    else:
        errors.append("node_getStatus mintingActive missing or not boolean")
    if plan["workload_kind"] == "payments":
        if transfer_source is None:
            errors.append(
                "payments manifest requires a reviewed transfer source; "
                "node_getStatus totalTransactions is not transfer evidence"
            )
        else:
            transfers, transfer_errors = transfer_source
            errors.extend(str(error) for error in transfer_errors)
            if is_count(transfers):
                observation["confirmed_transfers"] = transfers
            elif not transfer_errors:
                errors.append("reviewed transfer source gave no transfer count")
    elif transfer_source is not None:
        errors.append("a reviewed transfer source is only valid for a payments manifest")
    elif is_count(status.get("totalTransactions")):
        observation["confirmed_transfers"] = status["totalTransactions"]
    else:
        errors.append("node_getStatus totalTransactions missing or malformed")
    if expected_network is not None and status.get("network") != expected_network:
        errors.append(
            f"node_getStatus network {status.get('network')!r} "
            f"is not the declared {expected_network!r}"
        )
    commit = status.get("gitCommit")
    revision = plan["source_revision"]
    if commit is None or commit in ("", "unknown"):
        pass  # dev build without git metadata: nothing to contradict
    elif not isinstance(commit, str) or not COMMIT.fullmatch(commit):
        errors.append("node_getStatus gitCommit is not a >=7-hex commit id")
    elif not COMMIT.fullmatch(revision):
        errors.append(
            "manifest source_revision is not a >=7-hex commit id; "
            "cannot check node_getStatus gitCommit"
        )
    elif not (revision.startswith(commit) or commit.startswith(revision)):
        errors.append("node_getStatus gitCommit differs from manifest source_revision")
    return observation


def read_transfer_source(path, plan, node, now):
    """Read one reviewed transfer-count file; returns (confirmed_transfers, errors).

    The file is written by a separately reviewed canonical transfer counter
    for the declared payments workload on this chain::

        {"source": "<reviewed counter id>", "observed_at_unix_s": <float>,
         "chain_id": "<manifest chain_id>", "confirmed_transfers": <int>}

    Like an RPC reply it must be no older than ``max_gap_s`` and not in the
    future, so a stale count cannot masquerade as fresh evidence.
    """
    try:
        if path.stat().st_size > MAX_REPLY_BYTES:
            return None, ["reviewed transfer source exceeds size bound"]
        record = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        return None, [
            f"reviewed transfer source unreadable: {type(error).__name__}: {error}"
        ]
    if not isinstance(record, dict):
        return None, ["reviewed transfer source is not an object"]
    if not isinstance(record.get("source"), str) or not record["source"]:
        return None, ["reviewed transfer source names no source"]
    observed = record.get("observed_at_unix_s")
    if (
        not isinstance(observed, (int, float))
        or isinstance(observed, bool)
        or observed != observed
        or not 0 <= now - observed <= plan["max_gap_s"]
    ):
        return None, ["reviewed transfer source timestamp stale, future or malformed"]
    if record.get("chain_id") != node["chain_id"]:
        return None, ["reviewed transfer source chain_id is not the manifest node's chain"]
    if not is_count(record.get("confirmed_transfers")):
        return None, ["reviewed transfer source confirmed_transfers malformed"]
    return record["confirmed_transfers"], []


def fetch_status(url, timeout, context=None, opener=urllib.request.urlopen):
    """One bounded JSON-RPC ``node_getStatus`` call; returns (result, errors)."""
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": "node_getStatus", "params": {}}
    ).encode()
    request = urllib.request.Request(
        url, data=body, headers={"Content-Type": "application/json"}, method="POST"
    )
    try:
        with opener(request, timeout=timeout, context=context) as response:
            raw = response.read(MAX_REPLY_BYTES + 1)
        if len(raw) > MAX_REPLY_BYTES:
            return None, ["node_getStatus reply exceeds size bound"]
        reply = json.loads(raw)
    except (OSError, ValueError, http.client.HTTPException) as error:
        # HTTPException (BadStatusLine, IncompleteRead, ...) is not an
        # OSError; it must still become an error observation, never a crash
        # that leaves no file (#1743).
        return None, [f"node_getStatus request failed: {type(error).__name__}: {error}"]
    if not isinstance(reply, dict) or reply.get("error") is not None:
        return None, [
            f"node_getStatus RPC error: {reply.get('error') if isinstance(reply, dict) else reply!r}"
        ]
    return reply.get("result"), []


def write_atomically(path, value):
    """Replace the observation file whole so the sampler never reads a partial one."""
    path = Path(path)
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, prefix=path.name + ".")
    try:
        with os.fdopen(descriptor, "w") as output:
            json.dump(value, output, allow_nan=False)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def main(argv=None, *, now=time.time, opener=urllib.request.urlopen):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--node", required=True)
    parser.add_argument("--rpc-url", required=True, help=RPC_URL_HELP)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--expected-network")
    parser.add_argument("--ca-file", type=Path, help="pinned private CA for https")
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument(
        "--transfer-source",
        type=Path,
        help="reviewed transfer-count JSON file; required for a payments manifest",
    )
    args = parser.parse_args(argv)
    problem = rpc_url_problem(args.rpc_url, args.ca_file)
    if problem:
        parser.exit(2, f"{problem}\n")
    try:
        plan = validate_manifest(json.loads(args.manifest.read_text()))
        node = next(n for n in plan["nodes"] if n["node_id"] == args.node)
    except (OSError, ValueError, StopIteration) as error:
        parser.exit(2, f"manifest/node error: {error}\n")
    if not 0 < args.timeout <= plan["max_gap_s"]:
        parser.exit(2, "timeout must be positive and within max_gap_s\n")
    if args.transfer_source is not None and plan["workload_kind"] != "payments":
        parser.exit(2, "--transfer-source is only valid for a payments manifest\n")
    context = None
    if args.rpc_url.startswith("https://"):
        context = ssl.create_default_context(
            cafile=str(args.ca_file) if args.ca_file else None
        )
    status, errors = fetch_status(args.rpc_url, args.timeout, context, opener)
    # Timestamp at reply receipt: the sampler rejects anything older than
    # max_gap_s, so a slow reply cannot masquerade as fresh evidence.
    observed_at = now()
    transfer_source = None
    if plan["workload_kind"] == "payments" and args.transfer_source is not None:
        transfer_source = read_transfer_source(
            args.transfer_source, plan, node, observed_at
        )
    observation = observation_from_status(
        plan,
        node,
        status,
        observed_at,
        rpc_errors=errors,
        expected_network=args.expected_network,
        transfer_source=transfer_source,
    )
    write_atomically(args.out, observation)
    return 2 if observation["errors"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
