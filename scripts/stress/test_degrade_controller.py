"""Controller tolerance of plan-scheduled degrade steps (#1587).

A step named in ``degrade_steps`` (#1585) relaxes the fleet checks for its
named hosts only, and only inside ``[fault_start, window_end)``. Everything
else, including the same fault on an unnamed host or outside the window,
still halts. Fake RPC/observer coroutines; no network traffic.
"""

import asyncio
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from discovery import DiscoveryController
from plan import expand
from runtime import Gate, HOSTS, Journal, Quota

SUMMARY, _ = expand(
    json.loads(Path(__file__).with_name("x-series-plan.json").read_text())
)
STEPS = {s["id"]: s for s in SUMMARY["degrade_steps"]}
START = 1_000_000.0
GIB, MIB = 1024**3, 1024**2
A, B, N3, N4, N5 = HOSTS  # D1/D5 name A, D2/D5 name B, D3 N4, D4 N5
REFERENCE = {
    "at": 0, "pid": 1, "start": "start-1", "restarts": 0, "binary": "hash",
    "config": "config-digest", "node_key": "peer-key-digest",
    "disk_free": 10 * GIB, "disk_total": 20 * GIB, "mem_available": GIB,
    "mem_total": 4 * GIB, "swap": 0, "rss": 350 * MIB,
}
HASH = "aa" * 32


def at(step, key):
    """Absolute time of fault_start, restore_at or window_end for a step."""
    return START + STEPS[step][{"restore_at": "restore"}.get(key, key) + "_offset"]


class Fleet:
    """Five fake validators: RPC, observer export and a ~150 s observer ring."""

    def __init__(self, test):
        self.test = test
        self.height = {h: 100 for h in HOSTS}
        self.synced = {h: True for h in HOSTS}
        self.frozen = set()  # isolated: reachable, height does not advance
        self.rpc_down = set()  # every RPC raises (stopped / unreachable)
        self.fee_down = set()  # only fee_getRate raises
        self.stopped = set()  # observer reports error rows (no process)
        self.pid = {h: 1 for h in HOSTS}
        self.ring = {h: [] for h in HOSTS}
        self.receipt_fee = {h: 10 for h in HOSTS}
        self.unsubmitted = set()
        self.unconfirmed = set()
        self.unspent = set()  # chain_areKeyImagesSpent reports spent: False
        self.calls = []

    def advance(self):
        for host in HOSTS:
            if host not in self.frozen and host not in self.rpc_down:
                self.height[host] += 1

    def stop(self, host):
        self.rpc_down.add(host)
        self.stopped.add(host)

    def start(self, host, synced=True):
        self.rpc_down.discard(host)
        self.stopped.discard(host)
        self.pid[host] += 1
        self.synced[host] = synced
        self.height[host] = max(self.height[h] for h in HOSTS)

    async def rpc(self, host, method, params=None, **_):
        self.calls.append((host, method, json.dumps(params, sort_keys=True)))
        if host in self.rpc_down or (method == "fee_getRate" and host in self.fee_down):
            raise ConnectionError("connection refused: " + host)
        if method == "node_getStatus":
            return {"network": "botho-testnet", "gitCommit": "commit",
                    "version": "0.6.0", "synced": self.synced[host],
                    "chainHeight": self.height[host]}
        if method == "getBlockByHeight":
            h = params["height"]
            return {"hash": "genesis" if h == 0 else "block-%d" % h,
                    "timestamp": h, "txCount": 0}
        if method == "fee_getRate":
            return {"baseRate": 2, "baseMin": 1, "baseMax": 4,
                    "adjustmentActive": False, "congestion": 0}
        if method == "getTransactionStatus":
            if params["hash"] in self.unsubmitted:
                return {"status": "unknown"}
            return {"confirmed": params["hash"] not in self.unconfirmed,
                    "txHash": params["hash"]}
        if method == "getTransaction":
            return {"txHash": params["hash"], "blockHeight": 100,
                    "fee": self.receipt_fee[host], "outputCount": 1, "totalOutput": 1000}
        if method == "chain_areKeyImagesSpent":
            return [{"keyImage": k, "spent": host not in self.unspent, "pending": False}
                    for k in params["keyImages"]]
        if method == "chain_getOutputs":
            return [{"height": i, "outputs": []}
                    for i in range(params["start_height"], params["end_height"] + 1)]
        raise AssertionError("unexpected RPC: " + method)

    async def observer(self, host):
        self.calls.append((host, "observer", None))
        now = self.test.now
        if host in self.stopped:
            row = {"at": now, "error": "node process absent"}
        else:
            pid = self.pid[host]
            row = {**REFERENCE, "at": now, "pid": pid, "start": "start-%d" % pid}
        self.ring[host] = (self.ring[host] + [row])[-10:]
        return list(self.ring[host])


class DegradeControllerTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        asyncio.get_running_loop().set_debug(False)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.now = START
        self.fleet = Fleet(self)
        self.c = self.make()

    def make(self, steps=True):
        c = object.__new__(DiscoveryController)
        c.state = Path(self.tmp.name)
        c.j = Journal(c.state / "journal.sqlite", lambda: self.now)
        self.addCleanup(c.j.db.close)
        c.config = {"node_sha256": "hash", "infrastructure_end": START + 82 * 3600,
                    "resource_baselines": {h: dict(REFERENCE) for h in HOSTS}}
        c.plan = {"network": "botho-testnet", "node_commit": "commit",
                  "genesis": "genesis", "schema_version": 2, "seed": 7}
        c.degrade_steps = SUMMARY["degrade_steps"] if steps else []
        c.j.set("status", "running")
        c.j.set("setup_start", START - 8 * 3600)
        c.j.set("start", START)
        c.j.set("end", START + 72 * 3600)
        c.closed = False
        c.reconcile_wakeup = asyncio.Event()
        c.fresh = 0
        c.statuses = {}
        c.blocks = []
        c.wallets = []
        c.inventory = []
        c.spent = []
        c.inventory_initialized = False
        c.scan_lock = asyncio.Lock()
        c.admission_lock = asyncio.Lock()
        c.rpc = Mock(call=AsyncMock(side_effect=self.fleet.rpc))
        c.observer = AsyncMock(side_effect=self.fleet.observer)
        c.report = Mock()
        return c

    def clock(self):
        return patch("controller.time.time", lambda: self.now)

    async def monitor(self, until, actions=None):
        """Run monitor ticks every 15 s until ``until`` or a hold."""
        actions = dict(actions or {})

        async def sleep(_):
            self.now += 15
            self.fleet.advance()
            for when in sorted(actions):
                if when <= self.now:
                    actions.pop(when)()
            if self.now >= until or self.c.j.get("status") == "held":
                self.c.closed = True

        with (self.clock(), patch("controller.asyncio.sleep", side_effect=sleep),
              patch("controller.shutil.disk_usage", return_value=Mock(free=10 * GIB))):
            await self.c.monitor()

    def held(self):
        return self.c.j.get("status") == "held"

    def reason(self):
        return str(self.c.j.get("reason"))

    def accepted_row(self, name="pay", submitted=None):
        info = {"outputs": [{"index": 0, "amount": 1000, "target_key": "bb" * 32}],
                "key_images": ["image"], "wire_bytes": 123}
        self.c.j.offer(name, "campaign", (submitted or self.now) - 1, 0, 1, 1000)
        self.c.j.transition(name, "accepted", submitted=submitted or self.now,
                            hash=HASH, fee=10, info=info)
        self.c.inventory = [[], [{"utxo": {"tx_hash": [170] * 32, "output_index": 0,
                                           "amount": 1000, "target_key": [187] * 32}}]]

    async def reconcile(self, passes):
        self.c.sync = AsyncMock()
        self.c.accounting = Mock()
        left = [passes]

        async def wake(delay):
            self.now += max(delay, 0.2)
            self.fleet.advance()
            self.c.statuses = {h: {"chainHeight": self.fleet.height[h],
                                   "synced": self.fleet.synced[h]} for h in HOSTS}
            self.c.fresh = self.now
            left[0] -= 1
            if left[0] <= 0 or self.held():
                self.c.closed = True

        with (self.clock(), patch("controller.time.monotonic", lambda: self.now),
              patch.object(self.c, "wait_for_reconcile", create=True, side_effect=wake)):
            await self.c.reconcile()

    def settle(self):
        self.c.statuses = {h: {"chainHeight": self.fleet.height[h],
                               "synced": self.fleet.synced[h]} for h in HOSTS}
        self.c.fresh = self.now

    def calls_to(self, host, method=None):
        return [c for c in self.fleet.calls if c[0] == host and method in (None, c[1])]

    # -- helpers ------------------------------------------------------------

    def test_live_hosts_is_hosts_without_an_active_step(self):
        self.assertEqual(self.c.live_hosts(at("D1", "fault_start") - 1), HOSTS)
        self.assertEqual(self.c.live_hosts(at("D1", "fault_start")), HOSTS[1:])
        self.assertEqual(self.c.live_hosts(at("D5", "window_end")), HOSTS)
        view = self.c.fault_view(at("D5", "fault_start"))
        self.assertEqual((view["step"], view["expect"], view["expected_down"]),
                         ("D5", "safe_halt", (A, B)))
        self.assertEqual(self.make(steps=False).live_hosts(at("D1", "fault_start")), HOSTS)
        fresh = self.make()
        fresh.j.set("start", None)
        self.assertIsNone(fresh.fault_view(at("D1", "fault_start")))

    # -- negative: the relaxation is scoped to host and window --------------

    async def test_no_step_host_failure_halts_after_sixty_seconds(self):
        self.c = self.make(steps=False)
        self.now = at("D1", "fault_start") - 30
        await self.monitor(self.now + 300, {at("D1", "fault_start"): lambda: self.fleet.stop(A)})
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "monitor unavailable for sixty seconds")
        self.assertLess(self.now, at("D1", "fault_start") + 90)

    async def test_d1_failure_on_unnamed_host_still_halts(self):
        self.now = at("D1", "fault_start") - 30
        await self.monitor(self.now + 300, {at("D1", "fault_start"): lambda: self.fleet.stop(B)})
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "monitor unavailable for sixty seconds")

    async def test_outage_before_window_still_halts(self):
        self.now = at("D1", "fault_start") - 400
        await self.monitor(at("D1", "fault_start") + 60,
                           {at("D1", "fault_start") - 300: lambda: self.fleet.stop(A)})
        self.assertTrue(self.held())
        self.assertLess(self.now, at("D1", "fault_start"))

    async def test_outage_after_window_halts_with_rejoin_reason(self):
        self.now = at("D1", "fault_start") - 30
        await self.monitor(at("D1", "window_end") + 120,
                           {at("D1", "fault_start"): lambda: self.fleet.stop(A)})
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "D1: seed.botho.io failed to rejoin by window end")
        self.assertGreaterEqual(self.now, at("D1", "window_end"))
        self.assertLess(self.now, at("D1", "window_end") + 30)

    async def test_observer_error_row_outside_window_halts(self):
        self.now = at("D1", "window_end") + 600
        await self.monitor(self.now + 60, {self.now + 15: lambda: self.fleet.stopped.add(N3)})
        self.assertTrue(self.held())
        self.assertIn("observer cannot read required metrics", self.reason())

    async def test_fee_failure_on_unnamed_host_during_d1_halts(self):
        self.now = at("D1", "fault_start") + 60
        self.fleet.stop(A)
        self.fleet.fee_down.add(B)
        self.settle()
        await self.observe_fees_once()
        self.assertTrue(self.held())
        self.assertIn("fee evidence", self.reason())

    async def test_d1_unnamed_host_unsynced_still_halts(self):
        # The unsynced timer is suspended for the named host only; a
        # progress step (D1-D4) does not suspend it for anyone else.
        self.now = at("D1", "fault_start") - 30
        await self.monitor(at("D1", "fault_start") + 600,
                           {at("D1", "fault_start"): lambda: self.fleet.stop(A),
                            at("D1", "fault_start") + 60:
                                lambda: self.fleet.synced.update({N3: False})})
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "node unsynced for two minutes: " + N3)
        self.assertGreater(self.now, at("D1", "fault_start") + 180)
        self.assertLess(self.now, at("D1", "fault_start") + 240)

    # -- D1: named host stopped, then restarted -----------------------------

    async def run_d1(self, extra=None):
        self.now = at("D1", "fault_start") - 30
        restore = at("D1", "restore_at")
        actions = {at("D1", "fault_start"): lambda: self.fleet.stop(A),
                   restore: lambda: self.fleet.start(A, synced=False),
                   restore + 300: lambda: self.fleet.synced.update({A: True}),
                   **(extra or {})}
        await self.monitor(at("D1", "window_end") + 60, actions)

    async def test_d1_named_host_restart_runs_on_and_rejoins(self):
        gates = []

        def gate_probe():
            with self.clock():
                try:
                    self.c.gate()
                    gates.append("open")
                except Quota as error:
                    gates.append("quota: " + str(error))

        await self.run_d1({at("D1", "fault_start") + 120: gate_probe,
                           at("D1", "restore_at") + 60: gate_probe})
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(gates, ["open", "open"])  # admission continues
        baseline = self.c.j.get("baseline:" + A)
        self.assertEqual((baseline["pid"], baseline["start"], baseline["rebased_step"]),
                         (2, "start-2", "D1"))
        self.assertEqual(baseline["rss"], REFERENCE["rss"])  # launch RSS kept
        self.assertEqual(self.c.j.get("baseline:" + B), REFERENCE)
        self.assertIsNotNone(self.c.j.get("rejoin:D1"))
        self.assertTrue(self.c.statuses[A]["synced"])

    async def test_second_restart_in_one_step_halts(self):
        restore = at("D1", "restore_at")
        await self.run_d1({restore + 600: lambda: self.fleet.stop(A),
                           restore + 840: lambda: self.fleet.start(A)})
        self.assertTrue(self.held())
        self.assertEqual(self.reason(),
                         "D1: second node restart in one degrade step: seed.botho.io")

    async def test_named_host_behind_at_window_end_halts(self):
        self.now = at("D1", "fault_start") - 30
        restore = at("D1", "restore_at")
        actions = {at("D1", "fault_start"): lambda: self.fleet.stop(A),
                   restore: lambda: self.fleet.start(A),
                   restore + 15: lambda: self.fleet.frozen.add(A)}
        await self.monitor(at("D1", "window_end") + 60, actions)
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "D1: seed.botho.io failed to rejoin by window end")

    def test_unnamed_host_unsynced_still_pauses_admission(self):
        self.now = at("D1", "fault_start") + 60
        self.c.fresh = self.now
        self.c.statuses = {h: {"synced": h != B, "chainHeight": 100} for h in HOSTS}
        self.c.statuses[A] = {"expected_down": True}
        with self.clock(), self.assertRaises(Quota):
            self.c.gate()
        self.c.statuses[B]["synced"] = True
        with self.clock():
            self.c.gate()

    def test_d1_never_routes_a_new_payment_to_the_named_host(self):
        identifiers = ["degrade-%05d" % i for i in range(200)]
        self.now = at("D1", "fault_start") - 1
        with self.clock():
            before = {self.c.choose_ingress(i) for i in identifiers}
        self.now = at("D1", "fault_start") + 1
        with self.clock():
            during = [self.c.choose_ingress(i) for i in identifiers]
        self.assertIn(A, before)
        self.assertNotIn(A, during)
        self.assertEqual(set(during), set(HOSTS[1:]))

    async def test_d1_sync_never_reads_the_named_host(self):
        self.now = at("D1", "fault_start") + 60
        self.fleet.stop(A)
        self.fleet.height = {h: 249 for h in HOSTS}
        self.settle()
        self.c.statuses[A] = {"expected_down": True}
        self.c.wallets = [{"key": "k", "address": "addr"}]
        self.c.inventory = [[]]
        self.c.native = AsyncMock(return_value={
            "address": "addr", "owned": [{"id": "x:0", "key_image": "img"}]})
        with self.clock():
            self.assertEqual(await self.c.sync(), 249)
        outputs = [c[0] for c in self.fleet.calls if c[1] == "chain_getOutputs"]
        live = HOSTS[1:]
        self.assertEqual(outputs, [live[i % 4] for i in range(5)])
        spent = [c[0] for c in self.fleet.calls if c[1] == "chain_areKeyImagesSpent"]
        self.assertEqual(spent, [live[249 % 4]])
        self.assertEqual(self.calls_to(A), [])

    async def observe_fees_once(self):
        async def sleep(_):
            self.c.closed = True

        tip = min(s["chainHeight"] for h, s in self.c.statuses.items()
                  if "chainHeight" in s and not s.get("expected_down"))
        self.c.j.set("fee_block_height", tip - 3)
        with (self.clock(), patch("discovery.time.time", lambda: self.now),
              patch("discovery.asyncio.sleep", side_effect=sleep)):
            await self.c.observe_fees()

    async def test_d1_fee_observation_skips_only_the_named_host(self):
        self.now = at("D1", "fault_start") + 60
        self.fleet.stop(A)
        self.settle()
        self.c.statuses[A] = {"expected_down": True}
        await self.observe_fees_once()
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(self.calls_to(A), [])
        quoted = [c[0] for c in self.fleet.calls if c[1] == "fee_getRate"]
        self.assertEqual(quoted, list(HOSTS[1:]))
        blocks = [c[0] for c in self.fleet.calls if c[1] == "getBlockByHeight"]
        self.assertEqual(blocks, [HOSTS[1:][h % 4] for h in (98, 99, 100)])
        skipped = [json.loads(r[0]) for r in self.c.j.db.execute(
            "SELECT detail FROM events WHERE state='fee_quote_skipped'")]
        self.assertEqual([(s["host"], s["step"]) for s in skipped], [(A, "D1")])

    async def test_d1_prepared_row_status_check_skips_named_host(self):
        self.now = at("D1", "fault_start") + 60
        self.fleet.stop(A)
        self.settle()
        self.c.statuses[A] = {"expected_down": True}
        self.c.j.offer("prep", "campaign", self.now, 0, 1, 1000)
        self.c.j.db.execute("UPDATE intents SET state='prepared', hash=?, prepared=? WHERE id='prep'",
                            ("bb" * 32, self.now))
        self.fleet.unsubmitted.add("bb" * 32)
        self.c.submit_prepared = AsyncMock()
        with self.clock():
            await self.c.admission_tick()
        self.assertFalse(self.held(), self.reason())
        self.c.submit_prepared.assert_awaited_once_with("prep")
        statuses = [c[0] for c in self.fleet.calls if c[1] == "getTransactionStatus"]
        self.assertEqual(statuses, list(HOSTS[1:]))

    # -- confirmation on the live hosts, re-verified after rejoin -----------

    async def test_d1_confirms_on_live_hosts_then_verifies_named_host(self):
        self.now = at("D1", "fault_start") + 60
        self.fleet.stop(A)
        self.accepted_row()
        await self.reconcile(2)
        self.assertFalse(self.held(), self.reason())
        row = self.c.j.intent("pay")
        info = json.loads(row["info"])
        self.assertEqual(row["state"], "reconciled")
        self.assertEqual((info["deferred_hosts"], info["deferred_step"]), ([A], "D1"))
        self.assertEqual(set(info["confirmed_receipts"]), set(HOSTS[1:]))
        self.assertEqual(self.calls_to(A), [])
        # Rejoin: after window end the named host's receipt and spent state
        # are fetched, and the payment is fully verified on all five.
        self.now = at("D1", "window_end") + 1
        self.fleet.start(A)
        self.settle()
        self.c.closed = False
        await self.reconcile(1)
        self.assertFalse(self.held(), self.reason())
        info = json.loads(self.c.j.intent("pay")["info"])
        self.assertNotIn("deferred_hosts", info)
        self.assertEqual(set(info["confirmed_receipts"]), set(HOSTS))
        self.assertEqual(info["deferred_verified"]["hosts"], [A])
        self.assertEqual([c[1] for c in self.calls_to(A)],
                         ["getTransaction", "chain_areKeyImagesSpent"])
        self.assertEqual(self.c.deferred_rows(), [])

    async def test_disagreeing_receipt_from_rejoined_host_halts(self):
        self.now = at("D1", "fault_start") + 60
        self.fleet.stop(A)
        self.accepted_row()
        await self.reconcile(2)
        self.now = at("D1", "window_end") + 1
        self.fleet.start(A)
        self.settle()
        self.fleet.receipt_fee[A] = 11
        self.c.closed = False
        await self.reconcile(1)
        self.assertTrue(self.held())
        self.assertIn("fleet transaction receipt disagreement", self.reason())

    async def test_d1_unconfirmed_payment_halts_after_five_minutes(self):
        # Only safe_halt time is excluded from the 300 s clock; D1 is a
        # progress step, so its window still counts.
        self.now = at("D1", "fault_start") + 60
        submitted = self.now
        self.fleet.stop(A)
        self.accepted_row(submitted=submitted)
        self.fleet.unconfirmed.add(HASH)
        await self.reconcile(40)
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "payment unconfirmed after five minutes")
        self.assertGreater(self.now, submitted + 300)
        self.assertLessEqual(self.now, submitted + 390)  # one 30 s poll late at most
        self.assertLess(self.now, at("D1", "window_end"))

    async def test_spent_state_disagreement_on_rejoined_host_halts(self):
        self.now = at("D1", "fault_start") + 60
        self.fleet.stop(A)
        self.accepted_row()
        await self.reconcile(2)
        self.assertEqual(self.c.j.intent("pay")["state"], "reconciled")
        self.now = at("D1", "window_end") + 1
        self.fleet.start(A)
        self.settle()
        self.fleet.unspent.add(A)
        self.c.closed = False
        await self.reconcile(1)
        self.assertTrue(self.held())
        self.assertIn("spent-input state disagreement", self.reason())
        self.assertIn("deferred_hosts", json.loads(self.c.j.intent("pay")["info"]))

    async def test_spent_check_during_step_defers_the_named_host(self):
        # Confirmed on all five before D1, scanned during it: the spent
        # state on A is still owed and joins the post-window verification.
        self.now = at("D1", "fault_start") - 60
        self.accepted_row()
        self.c.statuses = {h: {"chainHeight": 99, "synced": True} for h in HOSTS}
        self.c.fresh = self.now
        self.c.sync = AsyncMock()
        self.c.accounting = Mock()

        async def wake(_):
            self.c.closed = True

        with (self.clock(), patch("controller.time.monotonic", lambda: self.now),
              patch.object(self.c, "wait_for_reconcile", create=True, side_effect=wake)):
            await self.c.reconcile()
        self.assertEqual(self.c.j.intent("pay")["state"], "confirmed")
        self.now = at("D1", "fault_start") + 30
        self.fleet.stop(A)
        self.settle()
        self.c.closed = False
        await self.reconcile(1)
        info = json.loads(self.c.j.intent("pay")["info"])
        self.assertEqual(self.c.j.intent("pay")["state"], "reconciled")
        self.assertEqual(set(info["confirmed_receipts"]), set(HOSTS))
        self.assertEqual((info["deferred_hosts"], info["deferred_step"]), ([A], "D1"))
        self.assertEqual(self.calls_to(A, "chain_areKeyImagesSpent"), [])

    async def test_deferred_verification_waits_while_host_is_named_again(self):
        # A D1 deferral cannot be verified while D5 names the same host.
        self.now = at("D1", "fault_start") + 60
        self.fleet.stop(A)
        self.accepted_row()
        await self.reconcile(2)
        self.now = at("D5", "fault_start") + 60
        self.fleet.calls.clear()
        self.c.closed = False
        await self.reconcile(1)
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(self.calls_to(A), [])
        self.assertEqual(len(self.c.deferred_rows()), 1)

    # -- D5: below quorum, safe halt ----------------------------------------

    def test_d5_pauses_admission_with_quota(self):
        self.now = at("D5", "fault_start") + 10
        self.settle()
        with self.clock(), self.assertRaisesRegex(Quota, "D5"):
            self.c.gate()
        self.now = at("D5", "window_end")
        self.settle()
        with self.clock():
            self.c.gate()

    async def test_d5_suspends_unsynced_timer_for_every_host(self):
        self.now = at("D5", "fault_start") - 30

        def lose_quorum():
            self.fleet.stop(A)
            self.fleet.stop(B)
            for host in (N3, N4, N5):
                self.fleet.synced[host] = False
                self.fleet.frozen.add(host)

        def restore():
            self.fleet.frozen.clear()
            self.fleet.start(A)
            self.fleet.start(B)
            for host in (N3, N4, N5):
                self.fleet.synced[host] = True

        await self.monitor(at("D5", "window_end") + 60,
                           {at("D5", "fault_start"): lose_quorum,
                            at("D5", "restore_at"): restore})
        self.assertFalse(self.held(), self.reason())
        self.assertIsNotNone(self.c.j.get("rejoin:D5"))
        self.assertEqual({self.c.j.get("baseline:" + h)["rebased_step"] for h in (A, B)},
                         {"D5"})

    async def test_d5_excludes_window_from_unconfirmed_timers(self):
        submitted = at("D5", "fault_start") - 100
        self.now = submitted
        self.accepted_row(submitted=submitted)
        self.fleet.unconfirmed.add(HASH)
        self.fleet.stop(A)
        self.fleet.stop(B)
        self.now = at("D5", "fault_start") + 30
        await self.reconcile(int((at("D5", "window_end") - self.now) // 30))
        self.assertFalse(self.held(), self.reason())
        self.assertLess(self.now, at("D5", "window_end") + 30)
        self.fleet.start(A)
        self.fleet.start(B)
        self.c.closed = False
        await self.reconcile(20)
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "payment unconfirmed after five minutes")
        # 100 s before the window plus >200 s after it: the 300 s clock resumed.
        self.assertGreater(self.now, at("D5", "window_end") + 200)
        self.assertLess(self.now, at("D5", "window_end") + 260)

    # -- D3: isolated, reachable but behind ---------------------------------

    async def run_d3(self, catch_up):
        self.now = at("D3", "fault_start") - 30
        checkpoint_heights = []

        def isolate():
            self.fleet.frozen.add(N4)
            self.fleet.synced[N4] = False

        def heal():
            self.fleet.frozen.discard(N4)
            self.fleet.synced[N4] = True
            if catch_up:
                self.fleet.height[N4] = self.fleet.height[N3]

        def probe():
            frozen = self.fleet.height[N4]
            checkpoint_heights.extend(
                json.loads(c[2])["height"] for c in self.fleet.calls
                if c[0] == N4 and c[1] == "getBlockByHeight")
            checkpoint_heights.append(("frozen", frozen))

        await self.monitor(at("D3", "window_end") + 60,
                           {at("D3", "fault_start"): isolate,
                            at("D3", "fault_start") + 600: probe,
                            at("D3", "restore_at"): heal})
        return checkpoint_heights

    async def test_d3_isolated_host_tolerated_then_rejoins(self):
        heights = await self.run_d3(catch_up=True)
        self.assertFalse(self.held(), self.reason())
        self.assertIsNotNone(self.c.j.get("rejoin:D3"))
        # Hash agreement kept including the isolated host at its own height.
        frozen = heights[-1][1]
        self.assertIn(frozen, heights[:-1])

    async def test_d3_isolated_host_not_caught_up_halts_after_window(self):
        await self.run_d3(catch_up=False)
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "D3: eu.seed.botho.io failed to rejoin by window end")
        self.assertGreaterEqual(self.now, at("D3", "window_end"))

    # -- phase boundary -----------------------------------------------------

    def deferred(self, step):
        self.c.j.offer("late", "campaign", self.now - 10, 0, 1, 1000)
        self.c.j.transition("late", "reconciled", info={
            "deferred_hosts": list(STEPS[step]["target_hosts"]), "deferred_step": step})

    async def campaign_tick(self, active, phase_start):
        self.c.plan.update(phases=[
            {"id": "recovery", "start_hour": 42, "end_hour": 48},
            {"id": "degrade", "start_hour": 48, "end_hour": 54},
            {"id": "repeat", "start_hour": 54, "end_hour": 72}])
        self.c.events = []
        self.c.j.set("active_phase", active)
        self.c.sync = AsyncMock()
        self.c.accounting = Mock()
        self.c.deliver = AsyncMock()
        self.now = START + phase_start * 3600 + 5
        with self.clock(), patch("discovery.time.time", lambda: self.now):
            await self.c.campaign_tick()

    async def test_deferred_receipts_at_phase_boundary_gate(self):
        self.now = at("D4", "fault_start") + 60
        self.deferred("D4")
        with self.assertRaisesRegex(Gate, "deferred step receipts unverified at phase boundary"):
            await self.campaign_tick("degrade", 54)

    async def test_step_open_at_its_own_phase_start_is_not_yet_due(self):
        # D1 starts with the degrade phase: a deferral it makes while the
        # 48 h boundary drains is verified after D1, not gated at 48 h.
        self.now = at("D1", "fault_start") + 2
        self.deferred("D1")
        await self.campaign_tick("recovery", 48)
        self.assertEqual(self.c.j.get("active_phase"), "degrade")

    # -- no-regression: no degrade_steps ------------------------------------

    async def test_without_degrade_steps_call_sequence_is_unchanged(self):
        """The pre-#1587 sequence, written from main's all-HOSTS formulas."""
        self.c = self.make(steps=False)
        self.now = at("D1", "fault_start") + 60  # would be inside D1
        await self.monitor(self.now + 30)
        self.assertFalse(self.held(), self.reason())
        expected = []
        for height, genesis in ((100, True), (101, False)):
            expected += [(h, "node_getStatus") for h in HOSTS]
            expected += [(h, "observer") for h in HOSTS]
            expected += [(h, "getBlockByHeight") for h in HOSTS]
            if genesis:
                expected += [(h, "getBlockByHeight") for h in HOSTS]
        self.assertEqual([c[:2] for c in self.fleet.calls], expected)
        self.assertEqual(self.c.statuses[A], {"network": "botho-testnet",
            "gitCommit": "commit", "version": "0.6.0", "synced": True, "chainHeight": 101})

        self.fleet.calls.clear()
        self.c.closed = False
        self.accepted_row()
        await self.reconcile(2)
        self.assertEqual(self.c.j.intent("pay")["state"], "reconciled")
        info = json.loads(self.c.j.intent("pay")["info"])
        self.assertNotIn("deferred_hosts", info)
        self.assertEqual([c[:2] for c in self.fleet.calls],
                         [(h, "getTransactionStatus") for h in HOSTS]
                         + [(h, "getTransaction") for h in HOSTS]
                         + [(h, "chain_areKeyImagesSpent") for h in HOSTS])

        self.fleet.calls.clear()
        self.fleet.height = {h: 249 for h in HOSTS}
        self.settle()
        self.c.wallets = [{"key": "k", "address": "addr"}]
        self.c.inventory = [[]]
        self.c.native = AsyncMock(return_value={
            "address": "addr", "owned": [{"id": "x:0", "key_image": "img"}]})
        del self.c.sync
        self.c.closed = False
        with self.clock():
            await self.c.sync()
        self.assertEqual([c[:2] for c in self.fleet.calls],
                         [(HOSTS[(first // 50) % 5], "chain_getOutputs")
                          for first in range(0, 250, 50)]
                         + [(HOSTS[249 % 5], "chain_areKeyImagesSpent")])

        self.fleet.calls.clear()
        await self.observe_fees_once()
        self.assertEqual([c[:2] for c in self.fleet.calls],
                         [(h, "fee_getRate") for h in HOSTS]
                         + [(HOSTS[h % 5], "getBlockByHeight") for h in (247, 248, 249)])
        import hashlib
        with self.clock():
            for i in range(20):
                name = "degrade-%05d" % i
                self.assertEqual(self.c.choose_ingress(name), HOSTS[int(hashlib.sha256(
                    ("7" + name).encode()).hexdigest(), 16) % 5])
        self.assertEqual(self.c.j.db.execute(
            "SELECT COUNT(*) FROM events WHERE state IN "
            "('fee_quote_skipped','expected_down_unreachable','rebaseline','deferred_verified')"
        ).fetchone()[0], 0)


if __name__ == "__main__":
    unittest.main()
