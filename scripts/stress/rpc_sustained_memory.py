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
"""

import argparse
import json
import os
from pathlib import Path
import re
import ssl
import tempfile
import time
import urllib.request

from sustained_memory import validate_manifest

HEX64 = re.compile(r"[0-9a-f]{64}")
# Loopback or explicit private-lab HTTP(S) endpoint ending in /rpc; no
# credentials, query strings or fragments. The runner ties this endpoint to
# the measured process before the experiment (runbook step 2).
RPC_URL = re.compile(
    r"https?://[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[1-9][0-9]{0,4})?/(?:rpc)?"
)
MAX_REPLY_BYTES = 256 * 1024


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
):
    """Map one ``node_getStatus`` result onto the sampler's observation JSON.

    ``pid``/``process_start_id``/``chain_id`` come from the immutable manifest
    node: the runner bound this endpoint to that process before t0, and the
    sampler re-verifies the live process identity on every row. The reply
    can only *contradict* identity (``network`` or ``gitCommit`` mismatch),
    never establish a new one.
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
    if is_count(status.get("totalTransactions")):
        observation["confirmed_transfers"] = status["totalTransactions"]
    else:
        errors.append("node_getStatus totalTransactions missing or malformed")
    if expected_network is not None and status.get("network") != expected_network:
        errors.append(
            f"node_getStatus network {status.get('network')!r} "
            f"is not the declared {expected_network!r}"
        )
    commit = status.get("gitCommit")
    if (
        isinstance(commit, str)
        and commit not in ("", "unknown")
        and not plan["source_revision"].startswith(commit)
        and not commit.startswith(plan["source_revision"])
    ):
        errors.append("node_getStatus gitCommit differs from manifest source_revision")
    return observation


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
    except (OSError, ValueError) as error:
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
    parser.add_argument("--rpc-url", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--expected-network")
    parser.add_argument("--ca-file", type=Path, help="pinned private CA for https")
    parser.add_argument("--timeout", type=float, default=5.0)
    args = parser.parse_args(argv)
    if not RPC_URL.fullmatch(args.rpc_url):
        parser.exit(2, "explicit http(s)://host[:port]/rpc URL required\n")
    try:
        plan = validate_manifest(json.loads(args.manifest.read_text()))
        node = next(n for n in plan["nodes"] if n["node_id"] == args.node)
    except (OSError, ValueError, StopIteration) as error:
        parser.exit(2, f"manifest/node error: {error}\n")
    if not 0 < args.timeout <= plan["max_gap_s"]:
        parser.exit(2, "timeout must be positive and within max_gap_s\n")
    context = None
    if args.rpc_url.startswith("https://"):
        context = ssl.create_default_context(
            cafile=str(args.ca_file) if args.ca_file else None
        )
    status, errors = fetch_status(args.rpc_url, args.timeout, context, opener)
    # Timestamp at reply receipt: the sampler rejects anything older than
    # max_gap_s, so a slow reply cannot masquerade as fresh evidence.
    observation = observation_from_status(
        plan,
        node,
        status,
        now(),
        rpc_errors=errors,
        expected_network=args.expected_network,
    )
    write_atomically(args.out, observation)
    return 2 if observation["errors"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
