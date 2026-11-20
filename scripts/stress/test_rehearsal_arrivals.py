import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from rehearsal_arrivals import audit, classify
from runtime import HOSTS, Journal, digest


class ArrivalTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.journal = Path(directory.name) / "journal.sqlite"
        self.plan = {"rehearsal": {"duration_seconds": 600, "interval_seconds": 60},
                     "limits": {"start_slot_grace_seconds": 120}}
        j = Journal(self.journal)
        with j.db:
            j.set("plan_digest", digest(self.plan))
            j.set("rehearsal_start", 1000)
            j.set("rehearsal_result", {"expected": 8, "offer_start": 1000,
                  "rolling_windows": {"intervals_per_window": 1, "nominal_span_seconds": 60},
                  "timing_tolerance": {"span_jitter_seconds": 1, "max_offer_lag_seconds": 5}})
            for i, lag in enumerate((0, 0, 2, 3, 4.875, 6, 0, 0)):
                name = f"rehearsal-{i:05d}"
                tx_hash = f"{i:064x}"
                offer = 1000 + 60 * i
                host = HOSTS[i % len(HOSTS)]
                receipt = dict(status="confirmed", txHash=tx_hash, fee=1, blockHeight=9,
                               outputCount=1, totalOutput=25)
                info = {"response_at": offer + lag + 0.25, "wire_bytes": 500, "hash": tx_hash,
                        "block_height": 9, "outputs": [{"amount": 25}],
                        "confirmed_receipts": dict.fromkeys(HOSTS, receipt)}
                j.offer(name, "rehearsal", offer, 0, 1, 10)
                j.transition(name, "reconciled", submitted=offer, finished=offer+10,
                             hash=tx_hash, fee=1, ingress=host, info=info)
                j.db.execute("INSERT INTO requests(at,host,method,bytes,status) VALUES (?,?,?,?,?)",
                             (offer+lag, host, "tx_submit", 500, "ok"))
        j.db.close()

    def test_inclusive_edges(self):
        for low, high, result in ((59, 61, "within"), (58, 59, "indeterminate"),
                                  (61, 62, "indeterminate"), (61.01, 62, "miss"),
                                  (58, 58.99, "miss")):
            self.assertEqual(classify(low, high, 59, 61), result)

    def test_separates_rpc_bounds_from_durable_markers(self):
        original = self.journal.read_bytes()
        result = audit(self.journal, self.plan)
        summary = result["summary"]
        self.assertEqual(summary["rolling_windows"], {"within": 2, "miss": 3, "indeterminate": 2})
        self.assertEqual(summary["controller_marker_window_misses"], 0)
        self.assertEqual(summary["validator_receipts"], 40)
        self.assertEqual(summary["offer_deadlines"], {"within": 6, "miss": 1, "indeterminate": 1})
        self.assertEqual(self.journal.read_bytes(), original)
        self.assertEqual(list(self.journal.parent.iterdir()), [self.journal])

    def test_refuses_uncheckpointed_copy_and_wrong_plan(self):
        with self.assertRaisesRegex(ValueError, "plan differs"):
            audit(self.journal, {**self.plan, "seed": 47})
        alias = self.journal.with_name("archive.sqlite")
        alias.symlink_to(self.journal)
        for suffix in ("-wal", "-journal"):
            sidecar = Path(str(self.journal) + suffix)
            sidecar.write_bytes(b"uncheckpointed")
            with self.assertRaisesRegex(ValueError, "checkpointed"):
                audit(alias, self.plan)
            sidecar.unlink()

    def test_refuses_reordered_or_ambiguous_submissions(self):
        for sql in ("UPDATE requests SET host='unknown' WHERE id=1",
                    "UPDATE requests SET at=at+7 WHERE id=1",
                    "UPDATE requests SET status='unknown' WHERE id=1",
                    "INSERT INTO requests(at,host,method,bytes,status) SELECT at,host,method,bytes,status FROM requests WHERE id=1",
                    "UPDATE intents SET submitted=1000.125 WHERE id='rehearsal-00001'"):
            original = self.journal.read_bytes()
            with sqlite3.connect(self.journal) as db:
                db.execute(sql)
            db.close()
            with self.assertRaises(ValueError):
                audit(self.journal, self.plan)
            self.journal.write_bytes(original)

    def test_refuses_missing_response_or_disagreeing_receipt(self):
        with sqlite3.connect(self.journal) as db:
            info = json.loads(db.execute("SELECT info FROM intents ORDER BY offered LIMIT 1").fetchone()[0])
            info["confirmed_receipts"][HOSTS[-1]]["fee"] += 1
            db.execute("UPDATE intents SET info=? WHERE id='rehearsal-00000'", (json.dumps(info),))
        db.close()
        with self.assertRaisesRegex(ValueError, "inconsistent receipt"):
            audit(self.journal, self.plan)
        del info["response_at"]
        with sqlite3.connect(self.journal) as db:
            db.execute("UPDATE intents SET info=? WHERE id='rehearsal-00000'", (json.dumps(info),))
        db.close()
        with self.assertRaisesRegex(ValueError, "missing or invalid timestamp"):
            audit(self.journal, self.plan)
