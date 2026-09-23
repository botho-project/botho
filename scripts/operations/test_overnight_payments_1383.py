"""Submission safety and success criteria; no real payments or network calls."""
import copy
import datetime as dt
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "runner", Path(__file__).with_name("overnight_payments_1383.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


def healthy():
    return {"nodes": [
        {"host": host, "genesis": runner.GENESIS,
         "status": {"synced": True, "network": "botho-testnet", "gitCommit": runner.COMMIT,
                    "chainHeight": 2637, "tipHash": "a" * 64},
         "transaction": {"confirmed": True}, "receipt": {"blockHeight": 2637}}
        for host in runner.HOSTS]}


class SubmissionTests(unittest.TestCase):
    def test_only_three_fixed_windows(self):
        for slot in runner.SLOTS:
            self.assertEqual(runner.due_slot(slot), slot)
            self.assertEqual(runner.due_slot(slot + dt.timedelta(seconds=119)), slot)
            self.assertIsNone(runner.due_slot(slot - dt.timedelta(seconds=1)))
            self.assertIsNone(runner.due_slot(slot + dt.timedelta(seconds=120)))
        self.assertIsNone(runner.due_slot(runner.SLOTS[0] + dt.timedelta(days=1)))

    def test_outside_window_cannot_request(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(runner, "rpc") as rpc:
            with patch.object(runner, "now", return_value=runner.SLOTS[-1] + dt.timedelta(hours=6)):
                runner.payment(Path(directory), Path("unused"))
            rpc.assert_not_called()

    def test_ambiguous_submission_is_never_retried(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            with patch.object(runner, "now", return_value=runner.SLOTS[0]), \
                    patch.object(runner, "preflight", return_value=("recipient", healthy())), \
                    patch.object(runner, "rpc", side_effect=TimeoutError("response lost")) as rpc:
                with self.assertRaises(TimeoutError):
                    runner.payment(state, Path("unused"))
                runner.payment(state, Path("unused"))
                self.assertEqual(rpc.call_count, 1)
            receipt = json.loads(next(state.glob("payment-*.json")).read_text())
            self.assertEqual(receipt["outcome"], "submission_unknown")

    def test_failed_preflight_consumes_slot_without_request(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(runner, "now", return_value=runner.SLOTS[0]), \
                    patch.object(runner, "preflight", side_effect=RuntimeError("wrong chain")), \
                    patch.object(runner, "rpc") as rpc:
                with self.assertRaises(RuntimeError):
                    runner.payment(Path(directory), Path("unused"))
                runner.payment(Path(directory), Path("unused"))
                rpc.assert_not_called()

    def test_three_slots_make_exactly_three_requests(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(runner, "preflight", return_value=("recipient", healthy())), \
                    patch.object(runner, "fleet_snapshot", return_value=healthy()), \
                    patch.object(runner, "rpc", return_value={
                        "success": True, "txHash": "b" * 64, "amount": str(runner.AMOUNT)}) as rpc:
                for slot in runner.SLOTS:
                    with patch.object(runner, "now", return_value=slot):
                        runner.payment(Path(directory), Path("unused"))
                        runner.payment(Path(directory), Path("unused"))
                self.assertEqual(rpc.call_count, 3)
            receipts = list(Path(directory).glob("payment-*.json"))
            self.assertEqual(len(receipts), 3)
            self.assertTrue(all(json.loads(p.read_text())["outcome"] == "confirmed_on_all_five"
                                for p in receipts))

    def test_late_preflight_does_not_send(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(runner, "now", side_effect=[runner.SLOTS[0], runner.SLOTS[0],
                              runner.SLOTS[0] + dt.timedelta(minutes=3), runner.SLOTS[0]]), \
                    patch.object(runner, "preflight", return_value=("recipient", healthy())), \
                    patch.object(runner, "rpc") as rpc:
                with self.assertRaises(RuntimeError):
                    runner.payment(Path(directory), Path("unused"))
                rpc.assert_not_called()

    def test_amount_or_identity_change_refuses_preflight(self):
        with patch.object(Path, "read_text", return_value="tbotho://2/test"), \
                patch.object(runner, "ADDRESS_SHA256", runner.hashlib.sha256(b"tbotho://2/test").hexdigest()):
            with patch.object(runner, "fleet_snapshot", return_value=healthy()), \
                    patch.object(runner, "rpc", return_value={"enabled": True, "amountPerRequest": runner.AMOUNT * 2}):
                with self.assertRaises(RuntimeError):
                    runner.preflight(Path("unused"))
            for field, bad in [("network", "botho-mainnet"), ("gitCommit", "other"), ("synced", False)]:
                snapshot = healthy()
                snapshot["nodes"][0]["status"][field] = bad
                with patch.object(runner, "fleet_snapshot", return_value=snapshot), patch.object(runner, "rpc") as rpc:
                    with self.assertRaises(RuntimeError):
                        runner.preflight(Path("unused"))
                    rpc.assert_not_called()

    def test_success_requires_five_matching_confirmations_and_sync(self):
        snapshot = healthy()
        self.assertTrue(runner.all_confirmed(snapshot))
        cases = [({"transaction": {"confirmed": False}}), ({"error": "timeout"}),
                 ({"receipt": {"blockHeight": 100}}),
                 ({"status": dict(snapshot["nodes"][0]["status"], synced=False)}),
                 ({"status": dict(snapshot["nodes"][0]["status"], tipHash="c" * 64)})]
        for change in cases:
            bad = copy.deepcopy(snapshot)
            bad["nodes"][0].update(change)
            self.assertFalse(runner.all_confirmed(bad))
        self.assertFalse(runner.all_confirmed({"nodes": snapshot["nodes"][:-1]}))


if __name__ == "__main__":
    unittest.main()
