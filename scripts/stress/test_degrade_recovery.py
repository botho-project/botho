"""Per-step recovery verdicts and time-to-rejoin measurements (#1591).

Fake RPC, metrics-server and observer coroutines drive the monitor through
scheduled degrade steps; ``Rpc.metrics`` is exercised against a loopback
HTTP server. No fleet traffic.
"""

import http.server
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import Mock

from reporting import controller_report, degrade_report, workload_complete
from runtime import (Gate, HOSTS, Journal, Quota, Rpc, loop_delta,
                     parse_loop_metrics)
import test_degrade_controller as base  # module import: do not re-collect its tests
from test_degrade_controller import STEPS, at

A, B, N3, N4, N5 = HOSTS

# Captured from the standalone metrics server (`metrics_port`) format: the
# Rust prometheus TextEncoder prints the 1.0 bucket as le="1". Other
# families on that server are present and must be ignored.
METRICS_SERVER = """\
# HELP botho_gossip_publish_errors_total Gossipsub publish() calls that returned an error, by topic
# TYPE botho_gossip_publish_errors_total counter
botho_gossip_publish_errors_total{topic="blocks"} 2
# HELP botho_node_loop_delayed_total Node event-loop iterations that exceeded the delay warning threshold
# TYPE botho_node_loop_delayed_total counter
botho_node_loop_delayed_total 3
# HELP botho_node_loop_iteration_seconds Duration of one node event-loop iteration including its handler
# TYPE botho_node_loop_iteration_seconds histogram
botho_node_loop_iteration_seconds_bucket{le="0.01"} 90210
botho_node_loop_iteration_seconds_bucket{le="0.05"} 98000
botho_node_loop_iteration_seconds_bucket{le="0.1"} 99100
botho_node_loop_iteration_seconds_bucket{le="0.25"} 99800
botho_node_loop_iteration_seconds_bucket{le="0.5"} 99950
botho_node_loop_iteration_seconds_bucket{le="1"} 99980
botho_node_loop_iteration_seconds_bucket{le="2.5"} 99992
botho_node_loop_iteration_seconds_bucket{le="5"} 99997
botho_node_loop_iteration_seconds_bucket{le="10"} 100000
botho_node_loop_iteration_seconds_bucket{le="30"} 100000
botho_node_loop_iteration_seconds_bucket{le="+Inf"} 100000
botho_node_loop_iteration_seconds_sum 812.5
botho_node_loop_iteration_seconds_count 100000
"""
# The RPC listener's /metrics (NodeMetrics.registry) has neither family.
RPC_METRICS = """\
# TYPE botho_peers_connected gauge
botho_peers_connected 4
"""


def scale(text, iterations, within, delayed):
    return (text.replace('le="+Inf"} 100000', 'le="+Inf"} %d' % iterations)
                .replace('le="1"} 99980', 'le="1"} %d' % within)
                .replace("delayed_total 3", "delayed_total %d" % delayed))


class LoopMetricsParserTests(unittest.TestCase):
    def test_metrics_server_text_parses_both_families(self):
        self.assertEqual(parse_loop_metrics(METRICS_SERVER),
                         {"iterations": 100000, "within_1s": 99980, "delayed": 3})
        # Python-style "1.0" bounds parse the same way.
        self.assertEqual(parse_loop_metrics(METRICS_SERVER.replace('le="1"', 'le="1.0"'))
                         ["within_1s"], 99980)

    def test_rpc_listener_metrics_are_refused(self):
        with self.assertRaisesRegex(Gate, "missing from metrics server"):
            parse_loop_metrics(RPC_METRICS)
        no_delayed = "\n".join(l for l in METRICS_SERVER.splitlines()
                               if not l.startswith("botho_node_loop_delayed_total"))
        with self.assertRaises(Gate):
            parse_loop_metrics(no_delayed)
        with self.assertRaises(Gate):
            parse_loop_metrics(METRICS_SERVER.replace("delayed_total 3", "delayed_total -1"))
        with self.assertRaises(Gate):
            parse_loop_metrics(METRICS_SERVER + 'botho_node_loop_iteration_seconds_bucket{le="1"} 5\n')

    def test_over_one_second_delta(self):
        before = parse_loop_metrics(METRICS_SERVER)
        after = parse_loop_metrics(scale(METRICS_SERVER, 100500, 100470, 3))
        # +Inf grew by 500, le="1" by 490: ten iterations over 1 s.
        self.assertEqual(loop_delta(before, after),
                         {"over_1s": 10, "delayed": 0, "reset": False})

    def test_delta_across_counter_reset(self):
        before = parse_loop_metrics(METRICS_SERVER)
        after = parse_loop_metrics(scale(METRICS_SERVER, 4000, 3996, 1))
        # Counters went backwards: count from zero to the post-restart value.
        self.assertEqual(loop_delta(before, after),
                         {"over_1s": 4, "delayed": 1, "reset": True})
        # A known restart counts from zero even when the new process has
        # already overtaken the old counts.
        bigger = parse_loop_metrics(scale(METRICS_SERVER, 200000, 199990, 4))
        self.assertEqual(loop_delta(before, bigger, restarted=True),
                         {"over_1s": 10, "delayed": 4, "reset": True})
        self.assertEqual(loop_delta(None, bigger)["delayed"], 4)


class MetricsHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.paths.append(self.path)
        if self.path == "/redir/metrics":
            self.send_response(302)
            self.send_header("Location", "/metrics")
            self.end_headers()
            return
        body = (b"x" * (8 * 1024 * 1024 + 1) if self.path == "/big/metrics"
                else METRICS_SERVER.encode())
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; version=0.0.4")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


class RpcMetricsTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.now = 1000.0
        self.j = Journal(Path(self.tmp.name) / "journal.sqlite", lambda: self.now)
        self.addCleanup(self.j.db.close)
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), MetricsHandler)
        self.server.paths = []
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        base = "http://127.0.0.1:%d" % self.server.server_address[1]
        prefix = {A: "", B: "/redir", N3: "/big"}
        targets = {h: {"rpc_url": base + prefix.get(h, "") + "/rpc"} for h in HOSTS}
        self.rpc = Rpc(self.j, targets, allow_env_proxy=False)

    def requests(self):
        return [tuple(r) for r in self.j.db.execute("SELECT host, method, status FROM requests")]

    async def test_reads_metrics_path_and_counts_against_quota(self):
        self.assertEqual(await self.rpc.metrics(A),
                         {"iterations": 100000, "within_1s": 99980, "delayed": 3})
        self.assertEqual(self.server.paths, ["/metrics"])
        self.assertEqual(self.requests(), [(A, "metrics", "ok")])

    async def test_refuses_redirects(self):
        with self.assertRaisesRegex(Gate, "redirect"):
            await self.rpc.metrics(B)
        self.assertEqual(self.server.paths, ["/redir/metrics"])  # never followed

    async def test_refuses_bodies_over_eight_mebibytes(self):
        with self.assertRaisesRegex(Gate, "metrics response byte bound"):
            await self.rpc.metrics(N3)

    async def test_exhausted_endpoint_quota_blocks_the_read(self):
        for _ in range(50):
            self.rpc.reserve(A, "node_getStatus", 1)
        with self.assertRaises(Quota):
            await self.rpc.metrics(A)
        self.assertEqual(self.server.paths, [])
        with self.assertRaisesRegex(Gate, "unlisted"):
            await self.rpc.metrics("localhost")


def status(height, synced=True, **extra):
    return {"network": "botho-testnet", "gitCommit": "commit", "version": "0.6.0",
            "synced": synced, "chainHeight": height, "tipHash": "block-%d" % height,
            "peerCount": 4, "scpPeerCount": 4, **extra}


class Base(unittest.IsolatedAsyncioTestCase):
    asyncSetUp = base.DegradeControllerTests.asyncSetUp
    make = base.DegradeControllerTests.make
    clock = base.DegradeControllerTests.clock
    monitor = base.DegradeControllerTests.monitor
    held = base.DegradeControllerTests.held
    reason = base.DegradeControllerTests.reason

    def record(self, step):
        return self.c.j.get("degrade:" + step)

    def verdict(self, step):
        return ((self.record(step) or {}).get("verdict") or {}).get("status")

    def quiescent_accounting(self, difference=0):
        """Accounting is exact whenever nothing is pending (the reconciler's rule)."""
        self.c.blocks = [{"height": 0, "outputs": []}]
        self.c.accounting = Mock(side_effect=lambda: self.c.j.set(
            "accounting", {"at": self.now, "difference": difference}))

    async def run_d1(self, extra=None, until=None):
        self.now = at("D1", "fault_start") - 30
        restore = at("D1", "restore_at")
        actions = {at("D1", "fault_start"): lambda: self.fleet.stop(A),
                   restore: lambda: self.fleet.start(A, synced=False),
                   restore + 300: lambda: self.fleet.synced.update({A: True}),
                   **(extra or {})}
        await self.monitor(until or at("D1", "window_end") + 60, actions)


class MilestoneTests(Base):
    async def test_synthetic_status_sequence_gives_expected_milestones(self):
        c, fs, r, we = self.c, at("D1", "fault_start"), at("D1", "restore_at"), at("D1", "window_end")
        self.quiescent_accounting()
        others = lambda height: [status(height) for _ in HOSTS[1:]]
        prior = {A: status(99, lastExternalizedSlot=99)}
        sequence = [
            (fs + 5, [None, *others(100)]),
            (r - 15, [None, *others(130)]),
            (r + 15, [status(90, synced=False), *others(131)]),       # reachable
            (r + 45, [status(120, synced=False), *others(133)]),
            (r + 75, [status(134, lastExternalizedSlot=99), *others(134)]),  # synced at tip
            (r + 105, [status(135, lastExternalizedSlot=135), *others(135)]),  # ballot
            (we + 5, [status(150), *others(150)]),
        ]
        c.j.set("baseline:" + A, {"rebased_step": "D1"})
        for when, statuses in sequence:
            self.now = when
            with self.clock():
                await c.degrade_evidence(when, statuses, prior)
        self.assertEqual(self.verdict("D1"), "pass")
        row = degrade_report(c.j, [STEPS["D1"]])[0]
        self.assertEqual(row["milestones"][A], {
            "first_reachable_at": 15, "synced_at_tip_at": 75,
            "first_ballot_at": 105, "first_coinbase_at": None})
        self.assertEqual(row["outage_blocks"], 31)  # 131 at restore - 100 at fault start
        self.assertAlmostEqual(row["catchup_blocks_per_second"][A], (134 - 90) / 60)
        self.assertEqual(row["fault_observed"], True)

    async def test_ballot_on_a_new_slot_counts_as_first_ballot(self):
        c, r = self.c, at("D1", "restore_at")
        others = [status(140) for _ in HOSTS[1:]]
        with self.clock():
            await c.degrade_evidence(at("D1", "fault_start"), [None, *others], {})
            await c.degrade_evidence(r, [None, *others], {})
            # A ballot on a slot at or below the restore height is history.
            await c.degrade_evidence(r + 15, [status(120, scpBallotCounter=2, scpSlotIndex=140), *others], {})
            self.assertIsNone(self.record("D1")["hosts"][A]["first_ballot_at"])
            await c.degrade_evidence(r + 30, [status(141, scpBallotCounter=1, scpSlotIndex=141), *others], {})
        self.assertEqual(self.record("D1")["hosts"][A]["first_ballot_at"], r + 30)


class VerdictTests(Base):
    async def test_d1_passes_with_every_criterion_met(self):
        self.quiescent_accounting()
        await self.run_d1()
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(self.verdict("D1"), "pass")
        record = self.record("D1")
        self.assertTrue(record["fault_observed"])
        self.assertEqual(record["loops"][A]["reset"], True)  # restart reset the counters
        self.assertEqual({h: record["loops"][h]["delayed"] for h in HOSTS}, dict.fromkeys(HOSTS, 0))
        self.assertEqual(set(record["tip_check"]["hosts"]), set(HOSTS))

    async def test_over_one_second_iterations_alone_do_not_fail(self):
        self.quiescent_accounting()

        def slow():
            self.fleet.loop[N4]["iterations"] += 7  # seven > 1 s, none >= 5 s
        await self.run_d1({at("D1", "restore_at") + 60: slow})
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(self.verdict("D1"), "pass")
        self.assertEqual(self.record("D1")["loops"][N4]["over_1s"], 7)
        row = degrade_report(self.c.j, [STEPS["D1"]])[0]
        self.assertEqual(row["loop"][N4], {"over_1s": 7, "delayed": 0})

    async def test_delayed_counter_increase_fails_the_step(self):
        self.quiescent_accounting()
        await self.run_d1({at("D1", "restore_at") + 60:
                           lambda: self.fleet.loop[N5].update(delayed=1)})
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "D1: recovery check failed: node_loop_delayed on " + N5)
        self.assertEqual(self.verdict("D1"), "fail")

    async def test_delayed_after_restart_on_the_target_fails(self):
        # Counted from the reset, not hidden by the lower post-restart value.
        self.quiescent_accounting()
        await self.run_d1({at("D1", "restore_at") + 60:
                           lambda: self.fleet.loop[A].update(delayed=1)})
        self.assertEqual(self.reason(), "D1: recovery check failed: node_loop_delayed on " + A)

    async def test_skipped_fault_does_not_pass(self):
        # The runner skipped D1 (skipped_late): no stop, no re-baseline.
        self.quiescent_accounting()
        self.now = at("D1", "fault_start") - 30
        await self.monitor(at("D1", "window_end") + 60)
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "D1: recovery check failed: fault not exercised")
        self.assertEqual(self.verdict("D1"), "fail")

    async def test_isolate_never_observed_fails_and_observed_passes(self):
        self.quiescent_accounting()
        self.now = at("D3", "fault_start") - 30
        self.c.j.set("degrade:D1", {"verdict": {"status": "pass"}})
        self.c.j.set("degrade:D2", {"verdict": {"status": "pass"}})
        await self.monitor(at("D3", "window_end") + 60)  # never isolated
        self.assertEqual(self.reason(), "D3: recovery check failed: fault not exercised")

    async def test_isolate_observed_by_peer_count_passes(self):
        self.quiescent_accounting()
        self.now = at("D3", "fault_start") - 30
        self.c.j.set("degrade:D1", {"verdict": {"status": "pass"}})
        self.c.j.set("degrade:D2", {"verdict": {"status": "pass"}})

        def heal():
            self.fleet.frozen.discard(N4)
            self.fleet.height[N4] = self.fleet.height[N3]
        await self.monitor(at("D3", "window_end") + 60,
                           {at("D3", "fault_start"): lambda: self.fleet.frozen.add(N4),
                            at("D3", "restore_at"): heal})
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(self.verdict("D3"), "pass")
        self.assertTrue(self.record("D3")["fault_observed"])

    async def test_netem_fault_observed_is_unknown_and_not_a_failure(self):
        self.quiescent_accounting()
        self.now = at("D4", "fault_start") - 30
        for step in ("D1", "D2", "D3"):
            self.c.j.set("degrade:" + step, {"verdict": {"status": "pass"}})
        await self.monitor(at("D4", "window_end") + 60)
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(self.record("D4")["fault_observed"], "unknown")
        self.assertEqual(self.verdict("D4"), "pass")

    async def test_tip_mismatch_in_the_verdict_names_the_step(self):
        self.quiescent_accounting()
        self.now = at("D1", "fault_start") - 30
        await self.run_d1(until=at("D1", "window_end") - 30)
        self.assertIsNone(self.verdict("D1"))
        rpc = self.fleet.rpc
        verdict_tick = []

        async def forked(host, method, params=None, **kw):
            result = await rpc(host, method, params, **kw)
            if method == "getBlockByHeight" and host == N4 and verdict_tick:
                result = {**result, "hash": "fork"}
            return result
        self.c.rpc.call.side_effect = forked
        self.c.closed = False
        statuses = [status(self.fleet.height[h]) for h in HOSTS]
        verdict_tick.append(True)
        self.now = at("D1", "window_end")
        with self.clock(), self.assertRaisesRegex(
                Gate, "D1: recovery check failed: one tip: conflicting hashes"):
            await self.c.degrade_evidence(self.now, statuses, self.c.statuses)
        self.assertEqual(self.verdict("D1"), "fail")

    async def test_accounting_mismatch_fails(self):
        self.quiescent_accounting(difference=1)
        await self.run_d1()
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "D1: recovery check failed: accounting difference 1")

    async def test_accounting_gate_fails(self):
        self.c.blocks = [{"height": 0, "outputs": []}]
        self.c.accounting = Mock(side_effect=Gate("exact integer accounting mismatch"))
        await self.run_d1()
        self.assertEqual(self.reason(),
                         "D1: recovery check failed: accounting: exact integer accounting mismatch")

    async def test_accounting_never_established_stays_open(self):
        # No quiescent snapshot after window_end: never a pass.
        await self.run_d1()
        self.assertFalse(self.held(), self.reason())
        self.assertIsNone(self.verdict("D1"))
        row = degrade_report(self.c.j, [STEPS["D1"]])[0]
        self.assertEqual((row["verdict"], row["awaiting"]), ("open", ["accounting"]))

    async def test_skipped_in_window_offer_fails(self):
        self.quiescent_accounting()
        offered = at("D1", "restore_at") + 10

        def skip():
            self.c.j.offer("degrade-00001", "campaign", offered, 0, 1, 1000)
            self.c.j.transition("degrade-00001", "skipped", "generator_slot_expired",
                                finished=self.now)
        await self.run_d1({offered + 130: skip})
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "D1: recovery check failed: missed payment degrade-00001")

    async def test_in_window_payment_unresolved_after_window_end_fails(self):
        # Accounting is exact (nothing pending) but the open verdict still
        # owes the in-window payment; it then goes unresolved.
        self.quiescent_accounting()
        self.c.j.offer("pay", "campaign", at("D1", "restore_at"), 0, 1, 1000)
        await self.run_d1()
        self.assertIsNone(self.verdict("D1"))
        self.c.j.transition("pay", "unresolved", "not reconciled within fifteen minutes")
        with self.clock(), self.assertRaisesRegex(
                Gate, "D1: recovery check failed: missed payment pay"):
            self.c.settle_degrade_verdicts(self.now)
        self.assertEqual(self.verdict("D1"), "fail")

    async def test_offer_outside_the_window_is_not_this_steps_miss(self):
        self.quiescent_accounting()
        self.c.j.offer("early", "campaign", at("D1", "fault_start") - 1, 0, 1, 1000)
        self.c.j.transition("early", "skipped", "generator_slot_expired", finished=self.now)
        await self.run_d1()
        self.assertEqual(self.verdict("D1"), "pass")

    async def test_unverified_deferred_receipt_never_passes(self):
        self.quiescent_accounting()

        def deferred():
            self.c.j.offer("pay", "campaign", at("D1", "fault_start") + 60, 0, 1, 1000)
            self.c.j.transition("pay", "reconciled", info={
                "deferred_hosts": [A], "deferred_step": "D1"})
        await self.run_d1({at("D1", "fault_start") + 90: deferred})
        self.assertFalse(self.held(), self.reason())
        self.assertIsNone(self.verdict("D1"))
        self.assertEqual(self.record("D1")["awaiting"], ["deferred receipts"])
        # Once verified (verify_deferred drops deferred_hosts) it passes.
        info = {"deferred_verified": {"step": "D1", "hosts": [A]}}
        self.c.j.db.execute("UPDATE intents SET info=? WHERE id='pay'", (json.dumps(info),))
        with self.clock():
            self.c.settle_degrade_verdicts(self.now)
        self.assertEqual(self.verdict("D1"), "pass")

    async def test_unreconciled_in_window_payment_keeps_the_verdict_open(self):
        self.quiescent_accounting()
        self.c.j.offer("pay", "campaign", at("D1", "restore_at"), 0, 1, 1000)
        await self.run_d1()
        # A planned row has spent nothing, so accounting is exact; the
        # unreconciled in-window offer alone keeps the verdict open.
        self.assertEqual(self.record("D1")["awaiting"], ["payments"])
        self.assertIsNone(self.verdict("D1"))

    async def run_d5(self, resume=True):
        self.quiescent_accounting()
        for step in ("D1", "D2", "D3", "D4"):
            self.c.j.set("degrade:" + step, {"verdict": {"status": "pass"}})
        self.now = at("D5", "fault_start") - 30

        def lose_quorum():
            self.fleet.stop(A)
            self.fleet.stop(B)
            for host in (N3, N4, N5):
                self.fleet.synced[host] = False
                self.fleet.frozen.add(host)

        def restore():
            self.fleet.start(A)
            self.fleet.start(B)
            for host in (N3, N4, N5):
                self.fleet.synced[host] = True
            if resume:
                self.fleet.frozen.clear()
            else:
                self.fleet.frozen.update((A, B))
        await self.monitor(at("D5", "window_end") + 60,
                           {at("D5", "fault_start"): lose_quorum,
                            at("D5", "restore_at"): restore})

    async def test_d5_safe_halt_with_resumption_passes(self):
        await self.run_d5()
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(self.verdict("D5"), "pass")
        self.assertIsNone(self.record("D5")["conflict"])

    async def test_d5_safe_halt_without_resumption_fails(self):
        await self.run_d5(resume=False)
        self.assertTrue(self.held())
        self.assertEqual(self.reason(), "D5: recovery check failed: tip did not advance after restore")

    async def test_d5_conflicting_hash_fails(self):
        rpc = self.fleet.rpc

        async def conflicting(host, method, params=None, **kw):
            result = await rpc(host, method, params, **kw)
            if (method == "node_getStatus" and host == N5
                    and at("D5", "fault_start") <= self.now < at("D5", "restore_at")):
                result = {**result, "tipHash": "other"}
            return result
        self.c.rpc.call.side_effect = conflicting
        await self.run_d5()
        self.assertTrue(self.held())
        self.assertIn("D5: recovery check failed: conflicting hash at height", self.reason())

    async def test_metrics_unavailable_on_a_live_host_is_not_a_pass(self):
        self.quiescent_accounting()
        await self.run_d1({at("D1", "window_end") - 60: lambda: self.fleet.metrics_down.add(N3)},
                          until=at("D1", "window_end") + 120)
        self.assertTrue(self.held())
        self.assertNotEqual(self.verdict("D1"), "pass")

    async def test_step_the_controller_never_saw_start_is_unobserved(self):
        self.quiescent_accounting()
        self.now = at("D2", "fault_start") + 120
        await self.monitor(self.now + 30)
        self.assertFalse(self.held(), self.reason())
        self.assertEqual(self.verdict("D1"), "unobserved")
        self.assertEqual(self.verdict("D2"), "unobserved")
        self.assertFalse(workload_complete(self.c.j, [], [], self.c.degrade_steps))


class CompletionAndReportTests(Base):
    def complete_journal(self):
        j = self.c.j
        j.set("status", "draining")
        j.set("phase_gates", {})
        j.set("accounting", {"difference": 0})
        for step in self.c.degrade_steps:
            j.set("degrade:" + step["id"], {"verdict": {"status": "pass"}})

    def test_workload_complete_requires_a_pass_for_every_step(self):
        self.complete_journal()
        steps = self.c.degrade_steps
        self.assertTrue(workload_complete(self.c.j, [], [], steps))
        for status in (None, "fail", "unobserved"):
            self.c.j.set("degrade:D4", {"verdict": status and {"status": status}})
            self.assertFalse(workload_complete(self.c.j, [], [], steps), status)
        self.c.j.set("degrade:D4", None)  # controller missed the verdict entirely
        self.assertFalse(workload_complete(self.c.j, [], [], steps))
        # Without degrade steps the rule adds nothing.
        self.assertTrue(workload_complete(self.c.j, [], []))

    def report(self, steps):
        self.c.degrade_steps = steps
        self.c.config.update(run_id="x1")
        self.c.events = []
        self.c.targets = {}
        (self.c.state / "artifacts").mkdir(exist_ok=True)
        self.c.j.limits["max_signed_transaction_bytes"] = 1024
        with self.clock():
            return controller_report(self.c)

    def test_empty_degrade_steps_never_reports_coverage(self):
        report = self.report([])  # rehearsal-only plans expand to []
        self.assertEqual(report["degrade"], [])
        self.assertIs(report["coverage"]["validator_degrade"], False)

    def test_coverage_true_only_when_every_step_passed(self):
        self.complete_journal()
        self.assertIs(self.report(self.c.degrade_steps)["coverage"]["validator_degrade"], True)
        self.c.j.set("degrade:D5", None)
        self.assertIs(self.report(self.c.degrade_steps)["coverage"]["validator_degrade"], False)

    def test_degrade_section_snapshot(self):
        r = at("D1", "restore_at")
        self.c.j.set("degrade:D1", {
            "restore_at": r, "closed_at": r + 1800, "fault_observed": True,
            "tip_fault_start": 100, "tip_restore": 131,
            "hosts": {A: {"first_reachable_at": r + 15, "synced_at_tip_at": r + 75,
                          "first_ballot_at": r + 105, "first_coinbase_at": None,
                          "reachable_height": 90, "synced_height": 135}},
            "loops": {h: {"over_1s": 2 if h == N4 else 0, "delayed": 0, "reset": h == A}
                      for h in HOSTS},
            "awaiting": [],
            "verdict": {"status": "pass", "at": r + 1830, "failures": []}})
        self.c.j.set("degrade:D2", {
            "restore_at": at("D2", "restore_at"), "closed_at": None,
            "tip_fault_start": 140, "tip_restore": None,
            "hosts": {B: {"first_reachable_at": None, "synced_at_tip_at": None,
                          "first_ballot_at": None, "first_coinbase_at": None,
                          "reachable_height": None, "synced_height": None}},
            "verdict": None})
        rows = self.report(self.c.degrade_steps)["degrade"]
        none = dict.fromkeys(("first_reachable_at", "synced_at_tip_at",
                              "first_ballot_at", "first_coinbase_at"))
        self.assertEqual(rows[:3], [
            {"step": "D1", "action": "stop_restart", "expect": "progress", "targets": [A],
             "verdict": "pass", "failures": [], "awaiting": [], "fault_observed": True,
             "milestones": {A: {"first_reachable_at": 15, "synced_at_tip_at": 75,
                                "first_ballot_at": 105, "first_coinbase_at": None}},
             "outage_blocks": 31, "catchup_blocks_per_second": {A: 0.75},
             "loop": {h: {"over_1s": 2 if h == N4 else 0, "delayed": 0} for h in HOSTS}},
            {"step": "D2", "action": "kill_restart", "expect": "progress", "targets": [B],
             "verdict": "in_progress", "failures": [], "awaiting": [], "fault_observed": None,
             "milestones": {B: none}, "outage_blocks": None,
             "catchup_blocks_per_second": {B: None}, "loop": {}},
            {"step": "D3", "action": "isolate", "expect": "progress", "targets": [N4],
             "verdict": "not_started", "failures": [], "awaiting": [], "fault_observed": None,
             "milestones": {}, "outage_blocks": None, "catchup_blocks_per_second": {},
             "loop": {}},
        ])
        self.assertEqual([r["step"] for r in rows], ["D1", "D2", "D3", "D4", "D5"])


if __name__ == "__main__":
    unittest.main()
