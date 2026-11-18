"""Open a prestaged rehearsal after its fixed start and verified prefunding."""

import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import sys
from tempfile import NamedTemporaryFile
import time

from runtime import Gate, atomic, digest, fsync_directory


def sha256(raw):
    return hashlib.sha256(raw).hexdigest()


def publish(path, config):
    """Concurrent starts and restarts must agree on the whole launch record."""
    if not path.exists():
        with NamedTemporaryFile(dir=path.parent) as stream:
            stream.write((json.dumps(config, sort_keys=True) + "\n").encode())
            stream.flush()
            os.fsync(stream.fileno())
            try:
                os.link(stream.name, path)
                fsync_directory(path.parent)
            except FileExistsError:
                pass
    if json.loads(path.read_bytes()) != config:
        raise Gate("existing launch differs from verified prefunding")


def confirmed_launch(launch, manifest, health_raw, balances_raw, checked_at):
    health = json.loads(health_raw)
    balances = json.loads(balances_raw)
    count = len(launch["wallets"])
    granted = manifest["grants_per_wallet"]
    amount = manifest["grant_picocredits"]
    accounting = health["accounting"]
    if (
        health["plan_sha256"] != manifest["plan_sha256"]
        or health["completed_before_deadline"] is not True
        or not 0 < health["at"] < manifest["deadline"]
        or health["at"] > checked_at
        or type(accounting["difference"]) is not int
        or accounting["difference"] != 0
        or accounting["opening"] + accounting["coinbase_lottery_inflow"]
        != accounting["remaining"] + accounting["principal"] + accounting["fees"]
        or accounting["principal"] != health["principal"]
        or accounting["fees"] != health["fees"]
    ):
        raise Gate("prefunder plan, deadline or accounting check failed")
    if (
        health["grants"] != count * granted
        or health["grant_picocredits"] != amount
        or health["principal"] != count * granted * amount
        or not isinstance(balances, list)
        or any(type(x) is not int for x in balances)
        or balances != [granted * amount] * count
        or balances != health["opening_balances"]
    ):
        raise Gate("prefunder grants and opening principal do not reconcile")
    return dict(
        launch,
        opening_balances=balances,
        prefund_evidence={
            "plan_sha256": manifest["plan_sha256"],
            "health_sha256": sha256(health_raw),
            "opening_balances_sha256": sha256(balances_raw),
        },
    )


def check_window(state, launch, setup_end, now):
    if now >= launch["infrastructure_end"]:
        raise Gate("fixed rehearsal window has expired")
    if now < setup_end:
        return
    # A published launch alone can survive a crash before ExecStart. Only the
    # normal controller records its launch digest in a journal it actually opened.
    journal = state / "journal.sqlite"
    recorded = state / "launch.json"
    if journal.is_file() and recorded.is_file():
        with closing(
            sqlite3.connect(journal.resolve().as_uri() + "?mode=ro", uri=True)
        ) as db:
            row = db.execute(
                "SELECT value FROM meta WHERE key='launch_digest'"
            ).fetchone()
        if row and json.loads(row[0]) == digest(json.loads(recorded.read_bytes())):
            return
    raise Gate("fixed rehearsal window has expired")


def handoff(state, pending):
    launch = pending["launch"]
    start = launch["setup_start"]
    stop = state / "STOP"
    if time.time() < start:
        raise Gate("fixed rehearsal setup has not started")
    manifest_raw = Path(pending["manifest"]).read_bytes()
    if sha256(manifest_raw) != pending["manifest_sha256"]:
        raise Gate("installed prefund manifest differs from staged pin")
    manifest = json.loads(manifest_raw)
    plan_raw = (state / "plan.json").read_bytes()
    if sha256(plan_raw) != manifest["plan_sha256"]:
        raise Gate("staged plan differs from prefund manifest")
    plan = json.loads(plan_raw)
    setup_end = start + plan["setup_deadline_hours"] * 3600
    output = state / "launch.json"
    funding = Path(manifest["state_dir"])
    while True:
        now = time.time()
        if stop.exists() or (funding / "STOP").exists():
            raise Gate("operator stop marker")
        check_window(state, launch, setup_end, now)
        try:
            balances = (funding / "opening_balances.json").read_bytes()
            health = (funding / "funding-health.json").read_bytes()
            config = confirmed_launch(launch, manifest, health, balances, time.time())
        except (FileNotFoundError, json.JSONDecodeError) as error:
            # The prefunder creates its exclusive final balance file after the
            # health report. An open file can briefly precede its first write.
            if now >= manifest["deadline"] or output.exists():
                raise Gate(
                    "missing or incomplete prefunder completion record"
                ) from error
            time.sleep(min(5, manifest["deadline"] - now, setup_end - now))
            continue
        check_window(state, launch, setup_end, time.time())
        config["prefund_evidence"]["manifest_sha256"] = pending["manifest_sha256"]
        publish(output, config)
        return


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", type=Path, required=True)
    args = parser.parse_args()
    try:
        pending = json.loads((args.state / "prefund-handoff.json").read_bytes())
        handoff(args.state, pending)
    except (Gate, OSError, ValueError, KeyError, TypeError, sqlite3.Error) as error:
        atomic(
            args.state / f"prefund-handoff-held-{time.time_ns()}.json",
            {"at": time.time(), "reason": str(error)},
        )
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
