#!/usr/bin/env python3
"""Bounded Sep 21–22, 2026 testnet experiment; deliberately not a daily job."""

import argparse
import concurrent.futures
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import time
import urllib.request

UTC = dt.timezone.utc
HOSTS = ["seed.botho.io", "seed2.botho.io", "faucet.botho.io",
         "eu.seed.botho.io", "ap.seed.botho.io"]
SLOTS = [dt.datetime(2026, 9, 22, hour, tzinfo=UTC) for hour in (2, 8, 14)]
OBSERVE_START = dt.datetime(2026, 9, 22, 1, tzinfo=UTC)
OBSERVE_END = dt.datetime(2026, 9, 22, 15, tzinfo=UTC)
COMMIT = "6dbcb92463214694f3122a05c9c48986d4f4f1b7"
GENESIS = "9ba39a7a724ce1c954d9cbf9b83c019e898a5129ef5f485c0488a3fad6d396df"
ADDRESS_SHA256 = "7fa9f604c2d993890ed2e10af848787d6bae6d01865fb2b0ff50b828554903bf"
AMOUNT = 1_000_000_000_000  # 1 testnet BTH; maximum three requests total.
DEFAULT_STATE = Path("/var/lib/botho-overnight-1383")
DEFAULT_ADDRESS = Path("/opt/botho-overnight-1383/address.txt")


def now():
    return dt.datetime.now(UTC)


def rpc(host, method, params=None, timeout=10):
    url = "http://127.0.0.1:17101/rpc" if host == "localhost" else f"https://{host}/rpc"
    request = urllib.request.Request(
        url, data=json.dumps({"jsonrpc": "2.0", "id": 1383, "method": method,
                             "params": params or {}}).encode(),
        headers={"Content-Type": "application/json"})
    # urllib's default TLS verification is required; writes are never retried.
    with urllib.request.urlopen(request, timeout=timeout) as response:
        body = json.load(response)
    if body.get("error") or not isinstance(body.get("result"), dict):
        raise RuntimeError(f"{host} {method}: {body.get('error', 'missing result')}")
    return body["result"]


def write_json(path, value):
    temporary = path.with_suffix(".tmp")
    with temporary.open("w") as output:
        json.dump(value, output, indent=2)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def append_json(path, value):
    with path.open("a") as output:
        fcntl.flock(output, fcntl.LOCK_EX)
        output.write(json.dumps(value) + "\n")
        output.flush()
        os.fsync(output.fileno())


def fleet_snapshot(tx_hash=None, identity=False):
    def probe(host):
        result = {"host": host, "observedAt": now().isoformat()}
        try:
            result["status"] = rpc(host, "node_getStatus")
            if identity:
                result["genesis"] = rpc(host, "getBlockByHeight", {"height": 0}).get("hash")
            if tx_hash:
                result["transaction"] = rpc(host, "getTransactionStatus", {"hash": tx_hash})
                if result["transaction"].get("confirmed"):
                    tx = rpc(host, "getTransaction", {"hash": tx_hash})
                    result["receipt"] = {k: tx.get(k) for k in
                                         ("blockHeight", "fee", "inputCount", "outputCount")}
        except Exception as error:
            result["error"] = str(error)
        return result
    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
        nodes = list(pool.map(probe, HOSTS))
    return {"observedAt": now().isoformat(), "nodes": nodes}


def preflight(address_path):
    address = address_path.read_text().strip()
    if not address.startswith("tbotho://2/") or hashlib.sha256(address.encode()).hexdigest() != ADDRESS_SHA256:
        raise RuntimeError("Unexpected recipient address")
    snapshot = fleet_snapshot(identity=True)
    for node in snapshot["nodes"]:
        status = node.get("status", {})
        if (node.get("error") or node.get("genesis") != GENESIS
                or status.get("network") != "botho-testnet"
                or status.get("gitCommit") != COMMIT or not status.get("synced")):
            raise RuntimeError(f"Preflight identity/health mismatch: {node['host']}")
    tips = {(node["status"]["chainHeight"], node["status"]["tipHash"])
            for node in snapshot["nodes"]}
    if len(tips) != 1:
        raise RuntimeError("Preflight fleet tips disagree")
    faucet = rpc("faucet.botho.io", "faucet_getStatus", timeout=60)
    if not faucet.get("enabled") or int(faucet.get("amountPerRequest", 0)) != AMOUNT:
        raise RuntimeError("Faucet is disabled or its grant amount changed")
    snapshot["faucet"] = faucet
    return address, snapshot


def due_slot(at):
    return next((slot for slot in SLOTS if 0 <= (at - slot).total_seconds() < 120), None)


def all_confirmed(snapshot):
    nodes = snapshot["nodes"]
    if len(nodes) != len(HOSTS):
        return False
    for node in nodes:
        status = node.get("status", {})
        if (node.get("error") or not node.get("transaction", {}).get("confirmed")
                or not status.get("synced") or status.get("network") != "botho-testnet"
                or status.get("gitCommit") != COMMIT
                or not re.fullmatch(r"[0-9a-f]{64}", status.get("tipHash", ""))
                or not isinstance(node.get("receipt", {}).get("blockHeight"), int)):
            return False
    return (len({node["status"].get("tipHash") for node in nodes}) == 1
            and len({node["receipt"]["blockHeight"] for node in nodes}) == 1)


def payment(state, address_path):
    slot = due_slot(now())
    if slot is None:
        print("Outside the three authorized two-minute start windows; no payment.")
        return
    name = slot.strftime("%Y%m%dT%H%M%SZ")
    path = state / f"payment-{name}.json"
    # A persistent exclusive marker is consumed even if preflight or submission
    # fails. A lost HTTP response must never cause an automatic duplicate grant.
    try:
        with path.open("x") as output:
            json.dump({"slot": slot.isoformat(), "outcome": "reserved"}, output)
            output.flush()
            os.fsync(output.fileno())
    except FileExistsError:
        print(f"Slot {name} already attempted; no payment.")
        return
    record = {"slot": slot.isoformat(), "startedAt": now().isoformat(),
              "addressSha256": ADDRESS_SHA256, "amountPicocredits": str(AMOUNT),
              "outcome": "preflight"}
    write_json(path, record)
    started = None
    try:
        address, before = preflight(address_path)
        append_json(state / f"fleet-{name}.jsonl", before)
        # Refuse a late preflight completion rather than compressing the cadence.
        if due_slot(now()) != slot:
            raise RuntimeError("Preflight exceeded authorized start window")
        record.update(outcome="submission_unknown", requestStartedAt=now().isoformat())
        write_json(path, record)
        started = time.monotonic()
        response = rpc("faucet.botho.io", "faucet_request", {"address": address}, timeout=90)
        record.update(response=response, responseAt=now().isoformat())
        tx_hash = response.get("txHash", "")
        if not response.get("success") or not re.fullmatch(r"[0-9a-f]{64}", tx_hash):
            record["outcome"] = "request_rejected"
            write_json(path, record)
            raise RuntimeError("Faucet did not return a successful transaction")
        if int(response.get("amount", 0)) != AMOUNT:
            raise RuntimeError("Faucet returned an unexpected amount")
        record.update(outcome="submitted", txHash=tx_hash)
        write_json(path, record)
        deadline = started + 15 * 60
        while time.monotonic() < deadline:
            snapshot = fleet_snapshot(tx_hash)
            append_json(state / f"fleet-{name}.jsonl", snapshot)
            if all_confirmed(snapshot):
                record.update(outcome="confirmed_on_all_five", confirmedAt=now().isoformat(),
                              confirmationSeconds=round(time.monotonic() - started, 3),
                              finalSnapshot=snapshot)
                write_json(path, record)
                print(json.dumps({k: record[k] for k in
                                  ("slot", "outcome", "txHash", "confirmationSeconds")}))
                return
            time.sleep(15)
        record["outcome"] = "confirmation_timeout"
        raise RuntimeError("Not confirmed and synced on all five within 15 minutes")
    except Exception as error:
        if record["outcome"] == "preflight":
            record["outcome"] = "preflight_failed"
        record.update(error=str(error), finishedAt=now().isoformat())
        write_json(path, record)
        raise


def local_sample(state):
    if not OBSERVE_START <= now() < OBSERVE_END:
        print("Observation window ended; no sample.")
        return
    record = {"observedAt": now().isoformat(), "host": socket.getfqdn()}
    try:
        unit = subprocess.check_output([
            "systemctl", "show", "botho.service", "-p", "MainPID", "-p", "NRestarts",
            "-p", "ActiveState", "-p", "MemoryCurrent", "-p", "MemoryPeak",
            "-p", "MemorySwapCurrent", "-p", "ExecMainStartTimestamp"], text=True, timeout=10)
        record["service"] = dict(line.split("=", 1) for line in unit.strip().splitlines())
        pid = record["service"].get("MainPID", "0")
        if pid != "0":
            fields = dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines() if ":" in line)
            record["process"] = {key: fields.get(key, "").strip() for key in
                                 ("VmRSS", "VmHWM", "VmSwap", "Threads")}
        record["status"] = rpc("localhost", "node_getStatus")
    except Exception as error:
        record["error"] = str(error)
    append_json(state / "health.jsonl", record)
    print(json.dumps(record))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["check", "pay", "sample"])
    parser.add_argument("--state", type=Path, default=DEFAULT_STATE)
    parser.add_argument("--address", type=Path, default=DEFAULT_ADDRESS)
    args = parser.parse_args()
    os.umask(0o077)
    args.state.mkdir(mode=0o700, parents=True, exist_ok=True)
    if args.mode == "check":
        _, snapshot = preflight(args.address)
        write_json(args.state / "preflight.json", snapshot)
        print("Read-only preflight passed: recipient, five HTTPS ingresses, chain/build and 1-BTH faucet.")
    elif args.mode == "sample":
        local_sample(args.state)
    else:
        with (args.state / "payment.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            payment(args.state, args.address)


if __name__ == "__main__":
    main()
