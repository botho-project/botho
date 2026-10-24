"""max_inflight counts only unconfirmed work; confirmed-but-unscanned rows keep
their sender reserved until reconciliation (#1551). No live network traffic."""

import asyncio
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, patch

from controller import Controller
from runtime import INFLIGHT, PENDING, Gate, Journal

CAP = 32


def confirm_burst(journal, count, first_sender=0, state="confirmed"):
    """`count` payments confirmed on every host whose wallet scan is still due."""
    for i in range(count):
        name = "done-" + str(i)
        journal.offer(name, "campaign", 900, first_sender + i, 100 + i, 10)
        journal.transition(name, state)


class InflightSetTests(unittest.TestCase):
    def setUp(self):
        from test_discovery import DiscoveryTests

        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.limits = DiscoveryTests().profile()["limits"]
        self.j = Journal(
            Path(self.tmp.name) / "journal.sqlite", lambda: 1000, limits=self.limits
        )
        self.addCleanup(lambda: self.j.db.close())

    def prepare(self, name, sender):
        self.j.offer(name, "campaign", 1000, sender, sender + 1, 100)
        self.j.transition(name, "eligible")
        info = {"hash": name.encode().hex().rjust(64, "0"), "fee": 100_000_000,
                "bytes": 5000, "selected": [{"id": "input-" + name}]}
        self.j.prepare(name, info, CAP, 1120)

    def test_inflight_is_the_unconfirmed_subset_of_pending(self):
        self.assertEqual(set(PENDING) - set(INFLIGHT), {"confirmed", "recipient_verified"})
        confirm_burst(self.j, 3)
        self.j.offer("open", "campaign", 950, 50, 51, 10)
        self.j.transition("open", "accepted")
        self.assertEqual(len(self.j.pending()), 4)
        self.assertEqual([r["id"] for r in self.j.inflight()], ["open"])

    def test_journal_admits_past_confirmed_burst_but_not_their_senders(self):
        confirm_burst(self.j, CAP // 2)
        for i in range(CAP // 2, CAP):
            name = "done-" + str(i)
            self.j.offer(name, "campaign", 900, i, 100 + i, 10)
            self.j.transition(name, "recipient_verified")
        self.prepare("fresh", 40)
        self.assertEqual(self.j.intent("fresh")["state"], "prepared")
        with self.assertRaisesRegex(Gate, "wallet already reserved"):
            self.prepare("reuse", 0)
        self.assertEqual(self.j.intent("reuse")["state"], "eligible")

    def test_journal_still_caps_unconfirmed_work(self):
        confirm_burst(self.j, CAP, state="accepted")
        with self.assertRaisesRegex(Gate, "inflight limit"):
            self.prepare("over", 40)


class DiscoveryDeliverCapTests(unittest.TestCase):
    def setUp(self):
        from test_discovery import DiscoveryExecutorTests

        helper = DiscoveryExecutorTests()
        helper.setUp()
        self.addCleanup(helper.doCleanups)
        self.c = helper.c
        self.c.prepare = AsyncMock(return_value=True)
        self.c.materialize = lambda *args: None

    def queue(self, name, sender, offered):
        self.c.j.offer(name, "campaign", offered, sender, sender + 1, 10)
        return {"intent": name, "max_inflight": CAP, "input_count": 1,
                "fee_multiplier": 1, "cohort": "burst"}

    def test_burst_of_confirmed_unscanned_rows_still_admits_new_sender(self):
        confirm_burst(self.c.j, CAP)
        events = [self.queue("reuse", 0, 1000), self.queue("fresh", 40, 1001)]
        asyncio.run(self.c.deliver(events, 1000, "campaign"))
        # Sender 0 still has unscanned change: skipped, not prepared.
        self.assertEqual([c.args[0] for c in self.c.prepare.await_args_list], ["fresh"])

    def test_unconfirmed_rows_still_fill_the_cap(self):
        confirm_burst(self.c.j, CAP, state="accepted")
        events = [self.queue("fresh", 40, 1001)]
        asyncio.run(self.c.deliver(events, 1000, "campaign"))
        self.c.prepare.assert_not_awaited()


class LegacyCampaignCapTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.c = object.__new__(Controller)
        self.c.j = Journal(Path(self.tmp.name) / "journal.sqlite", lambda: 1000)
        self.addCleanup(lambda: self.c.j.db.close())
        self.c.j.set("start", 0)
        self.c.j.set("end", 10_000)
        self.c.j.set("active_phase", "burst")
        self.c.plan = {"phases": [{"id": "burst", "start_hour": 0, "end_hour": 2}]}
        self.c.prepare = AsyncMock(return_value=True)
        self.max_inflight = 4

    def event(self, index, sender):
        return {"intent": "campaign-" + str(index), "offset_seconds": 990 + index % 10,
                "sender": "wallet-" + str(sender + 1), "recipient": "wallet-8",
                "amount_picocredits": 10, "max_inflight": self.max_inflight,
                "scenario": "burst"}

    def tick(self):
        with patch("controller.time.time", return_value=1000):
            asyncio.run(self.c.campaign_tick())

    def test_burst_of_confirmed_unscanned_rows_still_admits(self):
        confirm_burst(self.c.j, self.max_inflight)
        self.c.events = [self.event(101, 40)]
        self.tick()
        self.assertEqual([c.args[0] for c in self.c.prepare.await_args_list],
                         ["campaign-101"])

    def test_confirmed_sender_is_refused_by_prepare(self):
        # campaign_tick defers sender safety to Controller.prepare, whose
        # pending() guard still includes confirmed-but-unscanned rows.
        del self.c.prepare
        self.c.gate = lambda: None
        self.c.sync = AsyncMock(side_effect=AssertionError("must not select inputs"))
        confirm_burst(self.c.j, self.max_inflight)
        self.c.events = [self.event(102, 0)]
        self.tick()
        self.assertEqual(self.c.j.intent("campaign-102")["state"], "planned")
        self.c.sync.assert_not_awaited()

    def test_unconfirmed_rows_still_fill_the_cap(self):
        confirm_burst(self.c.j, self.max_inflight, state="accepted")
        self.c.events = [self.event(103, 40)]
        self.tick()
        self.c.prepare.assert_not_awaited()


if __name__ == "__main__":
    unittest.main()
