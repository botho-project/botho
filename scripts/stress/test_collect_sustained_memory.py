"""The fixed-schedule collector drives the real adapter, sampler and analyzer.

Every test runs the actual collector loop against Linux ``/proc`` and cgroup v2
filesystem fixtures, with a fake clock and a fake ``node_getStatus`` endpoint.
No process is launched, no node is contacted and no time is slept.
"""

import contextlib
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import collect_sustained_memory as csm
import sample_sustained_memory
from sustained_memory import analyze, manifest_digest, validate_manifest

MIB = 1024 * 1024
T0 = 1_790_000_000
BINARY = b"fixture botho binary"
NODES = ("n1", "n2")  # n1 produces, n2 is a relay
PRODUCER = "n1"


class Clock:
    def __init__(self, start):
        self.time = float(start)

    def now(self):
        return self.time

    def sleep(self, seconds):
        assert seconds >= 0
        self.time += seconds


class Fleet:
    """Two local node processes plus a scripted node_getStatus endpoint.

    ``in_memory=False`` writes real Linux ``/proc`` and cgroup v2 fixture files
    on every tick, so the unmodified sampler reads them. That costs tens of
    thousands of file operations per five-hour profile, so most scenarios use
    ``in_memory=True``: the sampler's three filesystem readers (``identity``,
    ``process_metrics``, ``process_stat``) are replaced by readers of the same
    state, while its join/verification logic (``sample``) runs unchanged.
    """

    def __init__(
        self, root, clock, *, slope_mib_per_hour=0.0, keep_mining=False, in_memory=True
    ):
        self.root, self.clock = Path(root), clock
        self.in_memory = in_memory
        self.exe = {node: BINARY for node in NODES}
        self.state = {}
        self.slope = slope_mib_per_hour
        self.keep_mining = keep_mining
        self.proc = self.root / "proc"
        self.cgroups = self.root / "cgroups"
        self.machine_id = self.root / "machine-id"
        self.machine_id.write_text("host-1\n")
        boot = self.proc / "sys/kernel/random"
        boot.mkdir(parents=True)
        (boot / "boot_id").write_text("boot\n")
        self.pids = {"n1": 101, "n2": 102}
        self.start_ticks = {"n1": 5000, "n2": 5001}
        self.stalls = {}  # tick time -> seconds the RPC call blocks
        self.outages = set()  # tick times whose RPC fails
        self.restart_at = None  # (node, time): process start ticks change
        for node, pid in self.pids.items():
            process = self.proc / str(pid)
            process.mkdir()
            (process / "exe").write_bytes(BINARY)
            (process / "cgroup").write_text(f"0::/botho-{node}.service\n")
            group = self.cgroups / f"botho-{node}.service"
            group.mkdir(parents=True)
            for name, data in {
                "memory.swap.current": "0",
                "cpu.stat": "usage_usec 1\nthrottled_usec 0\n",
                "memory.events": "high 0\nmax 0\noom 0\noom_kill 0\n",
                **{
                    f"{kind}.pressure": "some avg10=0.00 avg60=0.00 avg300=0.00 total=0"
                    for kind in ("cpu", "memory", "io")
                },
            }.items():
                (group / name).write_text(data)
            self.update(node)

    @property
    def paths(self):
        return {
            "proc": self.proc,
            "cgroups": self.cgroups,
            "machine_id": self.machine_id,
        }

    def elapsed(self):
        return self.clock.now() - T0

    def update(self, node):
        elapsed = self.elapsed()
        growth = self.slope * MIB * max(0.0, elapsed - 1800) / 3600
        rss = (int(300 * MIB + growth) // 1024) * 1024
        start = self.start_ticks[node]
        if (
            self.restart_at
            and self.restart_at[0] == node
            and elapsed >= self.restart_at[1]
        ):
            start += 1
        self.state[node] = {"rss": rss, "start": start, "cpu": max(0.0, elapsed)}
        if self.in_memory:
            return
        process = self.proc / str(self.pids[node])
        (process / "status").write_text(
            f"VmRSS:\t{rss // 1024} kB\nRssAnon:\t{(rss - 100 * MIB) // 1024} kB\nVmSwap:\t0 kB\n"
        )
        fields = ["0"] * 50
        utime = int(max(0.0, elapsed) * 100)
        fields[0], fields[11], fields[12], fields[19] = "S", str(utime), "0", str(start)
        (process / "stat").write_text(f"{self.pids[node]} (botho) " + " ".join(fields))
        (self.cgroups / f"botho-{node}.service/memory.current").write_text(
            str(rss + MIB)
        )

    def url(self, node):
        port = 19090 + (NODES.index(node) if node in NODES else 9)
        return f"http://127.0.0.1:{port}/rpc"

    def fetch(self, url, timeout, context):
        node = NODES[int(url.rsplit(":", 1)[1].split("/")[0]) - 19090]
        tick = round(self.elapsed())
        self.clock.time += self.stalls.pop(tick, 0.01)
        self.update(node)
        if tick in self.outages:
            return None, ["node_getStatus request failed: URLError: refused"]
        mint_end = 1800 + 14400
        minted = min(self.elapsed(), self.elapsed() if self.keep_mining else mint_end)
        height = 1 + int(max(0.0, minted) // 15)
        mining = node == PRODUCER and (self.keep_mining or self.elapsed() < mint_end)
        return {
            "network": "botho-testnet",
            "gitCommit": "c" * 40,
            "chainHeight": height,
            "tipHash": hashlib.sha256(str(height).encode()).hexdigest(),
            "mintingActive": mining,
            "totalTransactions": height,
        }, []

    def identity_record(self, node):
        with self.readers():
            return csm.identity(
                self.pids[node], self.proc, self.machine_id, self.cgroups
            )

    # In-memory replacements for the sampler's filesystem readers.
    def node_of(self, pid):
        return next(node for node, known in self.pids.items() if known == pid)

    def fake_stat(self, proc, pid):
        state = self.state[self.node_of(pid)]
        return str(state["start"]), state["cpu"]

    def fake_identity(self, pid, proc=None, machine_id=None, cgroups=None):
        node = self.node_of(pid)
        return {
            "pid": pid,
            "process_start_id": f"boot:{self.state[node]['start']}",
            "artifact_sha256": hashlib.sha256(self.exe[node]).hexdigest(),
            "host_id": "host-1",
            "cgroup_path": f"/sys/fs/cgroup/botho-{node}.service",
        }

    def fake_metrics(self, pid, proc=None, cgroups=None):
        node = self.node_of(pid)
        rss = self.state[node]["rss"]
        return {
            "rss_bytes": rss,
            "anonymous_bytes": rss - 100 * MIB,
            "swap_bytes": 0,
            "cpu_seconds": self.state[node]["cpu"],
            "cgroup_path": f"/sys/fs/cgroup/botho-{node}.service",
            "cgroup_memory_bytes": rss + MIB,
            "cgroup_swap_bytes": 0,
            "cgroup_memory_events": {"high": 0, "max": 0, "oom": 0, "oom_kill": 0},
            "cgroup_cpu_throttled_seconds": 0.0,
            "cgroup_cpu_stat": {"usage_usec": 1, "throttled_usec": 0},
            "cgroup_pressure": {},
        }

    @contextlib.contextmanager
    def readers(self):
        if not self.in_memory:
            yield
            return
        with (
            mock.patch.object(sample_sustained_memory, "identity", self.fake_identity),
            mock.patch.object(
                sample_sustained_memory, "process_metrics", self.fake_metrics
            ),
            mock.patch.object(sample_sustained_memory, "process_stat", self.fake_stat),
            mock.patch.object(csm, "identity", self.fake_identity),
        ):
            yield


def make_plan(fleet, **limits):
    return csm.build_manifest(
        campaign_id="q20261104-x2-memqual",
        source_revision="c" * 40,
        workload="minting",
        started_at=T0,
        chain_id="q20261104-x2-memqual",
        identities={node: fleet.identity_record(node) for node in NODES},
        producers={PRODUCER},
        min_blocks=10,
        min_transfers=0,
        limits=limits,
        now=T0 - 60,
    )


class Harness(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def fleet(self, start=T0 - 30, **kwargs):
        self.clock = Clock(start)
        fleet_root = self.root / "fleet"
        fleet_root.mkdir()
        return Fleet(fleet_root, self.clock, **kwargs)

    def run_collect(self, fleet, plan, nodes=NODES, out=None, stopped=lambda: False):
        out = out or self.root / "raw.jsonl"
        targets = [
            (n, fleet.url(n["node_id"]), None)
            for n in plan["nodes"]
            if n["node_id"] in nodes
        ]
        logs, self.fsyncs = [], []
        with fleet.readers(), csm.open_exclusive(out) as output:
            summary = csm.collect(
                plan,
                targets,
                output,
                now=self.clock.now,
                sleep=self.clock.sleep,
                fetch=fleet.fetch,
                stopped=stopped,
                expected_network="botho-testnet",
                log=logs.append,
                fsync=self.fsyncs.append,  # durability is asserted, not paid per tick
                **fleet.paths,
            )
        rows = [json.loads(line) for line in out.read_text().splitlines()]
        return summary, rows, logs


class ScheduleTests(unittest.TestCase):
    def test_minting_timeline_is_fixed_and_absolute(self):
        schedule = csm.profile_schedule("minting", T0)
        self.assertEqual(schedule["mint_until_unix_s"], T0 + 1800 + 14400)
        self.assertEqual(schedule["profile_end_unix_s"], T0 + 18030)
        self.assertEqual(schedule["tick_count"], 18030 // 5 + 1)
        self.assertEqual(
            [p["name"] for p in schedule["phases"]],
            ["warmup", "sustained", "settling", "idle"],
        )
        self.assertEqual(
            schedule["phases"][2]["start_unix_s"], schedule["mint_until_unix_s"]
        )
        self.assertLess(
            schedule["recommended_node_end_unix_s"],
            schedule["hard_lab_deadline_unix_s"],
        )
        self.assertGreater(
            schedule["recommended_node_end_unix_s"], schedule["profile_end_unix_s"]
        )
        self.assertEqual(csm.started_at_for_mint_until(T0 + 16200), T0)
        self.assertIsNone(csm.profile_schedule("payments", T0)["mint_until_unix_s"])

    def test_schedule_cli(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = csm.main(
                ["schedule", "--mint-until", str(T0 + 16200)], now=lambda: T0 - 600
            )
        self.assertEqual(code, 0)
        result = json.loads(out.getvalue())
        self.assertEqual(result["started_at_unix_s"], T0)
        self.assertEqual(result["seconds_until_start"], 600)
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(csm.main(["schedule"], now=lambda: T0), 2)
            self.assertEqual(
                csm.main(
                    ["schedule", "--workload", "payments", "--mint-until", "5"],
                    now=lambda: T0,
                ),
                2,
            )


class ManifestTests(Harness):
    def test_manifest_binds_actual_identities(self):
        fleet = self.fleet(in_memory=False)  # real /proc fixture files
        plan = make_plan(fleet)
        validate_manifest(plan)
        n1 = plan["nodes"][0]
        self.assertEqual(n1["process_start_id"], "boot:5000")
        self.assertEqual(n1["host_id"], "host-1")
        self.assertTrue(n1["producer"])
        self.assertFalse(plan["nodes"][1]["producer"])
        self.assertEqual(plan["artifact_sha256"], hashlib.sha256(BINARY).hexdigest())

    def test_refusals(self):
        fleet = self.fleet()
        identities = {node: fleet.identity_record(node) for node in NODES}
        base = dict(
            campaign_id="c",
            source_revision="c" * 40,
            workload="minting",
            started_at=T0,
            chain_id="chain",
            identities=identities,
            producers={PRODUCER},
            min_blocks=10,
            min_transfers=0,
        )
        with self.assertRaisesRegex(ValueError, "past"):
            csm.build_manifest(**base, now=T0 + 1)
        with self.assertRaisesRegex(ValueError, "ahead"):
            csm.build_manifest(**base, now=T0 - csm.MAX_LEAD_S - 1)
        with self.assertRaisesRegex(ValueError, "producer"):
            csm.build_manifest(**dict(base, producers={"n9"}), now=T0)
        mixed = dict(identities, n2=dict(identities["n2"], artifact_sha256="d" * 64))
        with self.assertRaisesRegex(ValueError, "same artifact"):
            csm.build_manifest(**dict(base, identities=mixed), now=T0)
        with self.assertRaisesRegex(ValueError, "at most"):
            csm.build_manifest(
                **base, limits={"slope_limit_mib_per_hour": 50.0}, now=T0
            )
        same_group = dict(
            identities,
            n2=dict(identities["n2"], cgroup_path=identities["n1"]["cgroup_path"]),
        )
        with self.assertRaisesRegex(ValueError, "cgroup"):
            csm.build_manifest(**dict(base, identities=same_group), now=T0)

    def test_manifest_cli_writes_once(self):
        fleet = self.fleet()
        args = ["manifest", "--out", str(self.root / "m.json"), "--campaign-id", "c"]
        args += [
            "--source-revision",
            "c" * 40,
            "--started-at",
            str(T0),
            "--chain-id",
            "chain",
        ]
        for node in NODES:
            path = self.root / f"{node}-identity.json"
            path.write_text(json.dumps(fleet.identity_record(node)))
            args += ["--node", f"{node}={path}"]
        args += ["--producer", PRODUCER, "--min-blocks", "10", "--slope-limit", "5"]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(csm.main(args, now=lambda: T0 - 60), 0)
        plan = json.loads((self.root / "m.json").read_text())
        self.assertEqual(plan["slope_limit_mib_per_hour"], 5.0)
        self.assertEqual(
            json.loads(out.getvalue())["manifest_sha256"], manifest_digest(plan)
        )
        before = (self.root / "m.json").read_bytes()
        with (
            contextlib.redirect_stderr(io.StringIO()),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(csm.main(args, now=lambda: T0 - 60), 2)
        self.assertEqual((self.root / "m.json").read_bytes(), before)


class CollectTests(Harness):
    def test_constant_trace_collects_complete_and_passes(self):
        fleet = self.fleet()
        plan = make_plan(fleet)
        summary, rows, logs = self.run_collect(fleet, plan)
        self.assertTrue(summary["complete"], logs)
        self.assertEqual(summary["ticks_sampled"], 18030 // 5 + 1)
        self.assertEqual(summary["rows_written"], 2 * summary["ticks_sampled"])
        self.assertEqual(summary["first_tick_unix_s"], T0)
        self.assertEqual(len(self.fsyncs), summary["ticks_sampled"])
        # Every row sits on the absolute grid; nothing before t0, nothing rebased.
        self.assertTrue(
            all(0 <= (r["observed_at_unix_s"] - T0) % 5 < 0.1 for r in rows)
        )
        self.assertGreaterEqual(min(r["observed_at_unix_s"] for r in rows), T0)
        result = analyze(plan, rows)
        self.assertEqual(
            result["status"],
            "passed",
            result["incomplete_reasons"] + result["failures"],
        )
        self.assertEqual(result["scope"], "limited_minting")
        self.assertGreater(result["same_height_comparisons"], 0)

    def test_real_proc_fixture_rows_are_complete_and_exact(self):
        """The unmodified sampler reads real /proc and cgroup files each tick."""
        fleet = self.fleet(in_memory=False)
        plan = make_plan(fleet)
        summary, rows, logs = self.run_collect(
            fleet, plan, stopped=lambda: self.clock.now() > T0 + 600
        )
        self.assertTrue(summary["interrupted"])
        self.assertEqual(summary["error_rows"], 0, logs)
        self.assertEqual(len(rows), 2 * (600 // 5 + 1))
        for row in rows:
            self.assertEqual(row["errors"], [])
            self.assertEqual(row["rss_bytes"], 300 * MIB)
            self.assertEqual(row["anonymous_bytes"], 200 * MIB)
            self.assertEqual(row["cgroup_memory_bytes"], 301 * MIB)
            self.assertEqual(row["artifact_sha256"], plan["artifact_sha256"])
            self.assertEqual(row["mining"], row["node_id"] == PRODUCER)
        result = analyze(plan, rows)
        # Partial coverage is diagnostic, never a pass.
        self.assertEqual(result["status"], "incomplete")
        self.assertEqual(result["failures"], [])

    def test_historical_49_mib_per_hour_fails(self):
        fleet = self.fleet(slope_mib_per_hour=49.0)
        plan = make_plan(fleet)
        summary, rows, _ = self.run_collect(fleet, plan)
        self.assertTrue(summary["complete"])
        result = analyze(plan, rows)
        self.assertEqual(result["status"], "failed")
        self.assertIn("n1: rss_plus_swap sustained growth exceeded", result["failures"])

    def test_tightened_gate_fails_a_slope_the_default_admits(self):
        fleet = self.fleet(slope_mib_per_hour=8.0)
        plan = make_plan(fleet, slope_limit_mib_per_hour=5.0)
        _, rows, _ = self.run_collect(fleet, plan)
        self.assertEqual(analyze(plan, rows)["status"], "failed")

    def test_miner_that_never_pauses_is_incomplete(self):
        fleet = self.fleet(keep_mining=True)
        plan = make_plan(fleet)
        _, rows, _ = self.run_collect(fleet, plan)
        result = analyze(plan, rows)
        self.assertEqual(result["status"], "incomplete")
        self.assertTrue(
            any("idle" in reason for reason in result["incomplete_reasons"])
        )

    def test_late_start_is_not_replayed_or_rebased(self):
        fleet = self.fleet(start=T0 + 600)
        plan = make_plan(fleet)
        summary, rows, _ = self.run_collect(fleet, plan)
        self.assertFalse(summary["complete"])
        self.assertGreaterEqual(min(r["observed_at_unix_s"] for r in rows), T0 + 600)
        result = analyze(plan, rows)
        self.assertEqual(result["status"], "incomplete")
        self.assertIn(
            "n1: warmup boundary coverage missing", result["incomplete_reasons"]
        )

    def test_stalled_tick_skips_slots_and_is_incomplete(self):
        fleet = self.fleet()
        fleet.stalls[3600] = 20.0
        plan = make_plan(fleet)
        summary, rows, logs = self.run_collect(fleet, plan)
        self.assertGreater(summary["ticks_skipped"], 0)
        self.assertFalse(summary["complete"])
        self.assertTrue(any("not replayed" in line for line in logs))
        result = analyze(plan, rows)
        self.assertEqual(result["status"], "incomplete")
        self.assertTrue(any("gap" in reason for reason in result["incomplete_reasons"]))

    def test_rpc_outage_is_recorded_and_collection_continues(self):
        fleet = self.fleet()
        fleet.outages.add(7200)
        plan = make_plan(fleet)
        summary, rows, _ = self.run_collect(fleet, plan)
        self.assertTrue(summary["reached_profile_end"])
        self.assertEqual(summary["error_rows"], 2)
        bad = [r for r in rows if r["errors"]]
        self.assertTrue(
            all("node_getStatus request failed" in r["errors"][0] for r in bad)
        )
        self.assertEqual(analyze(plan, rows)["status"], "incomplete")

    def test_process_restart_is_incomplete_never_a_new_baseline(self):
        fleet = self.fleet(slope_mib_per_hour=0.0)
        fleet.restart_at = ("n2", 9000)
        plan = make_plan(fleet)
        _, rows, _ = self.run_collect(fleet, plan)
        result = analyze(plan, rows)
        self.assertEqual(result["status"], "incomplete")
        self.assertIn(
            "n2: process restarted or PID changed", result["incomplete_reasons"]
        )

    def test_interrupt_keeps_written_rows(self):
        fleet = self.fleet()
        plan = make_plan(fleet)
        summary, rows, _ = self.run_collect(
            fleet, plan, stopped=lambda: self.clock.now() > T0 + 100
        )
        self.assertTrue(summary["interrupted"])
        self.assertFalse(summary["complete"])
        self.assertEqual(len(rows), summary["rows_written"])
        self.assertGreater(len(rows), 0)

    def test_per_host_files_join_in_the_analyzer(self):
        fleet = self.fleet()
        plan = make_plan(fleet)
        manifest = self.root / "manifest.json"
        manifest.write_text(json.dumps(plan))
        files = []
        for node in NODES:
            self.clock.time = T0 - 30
            out = self.root / f"{node}.jsonl"
            summary, _, _ = self.run_collect(fleet, plan, nodes=(node,), out=out)
            self.assertTrue(summary["complete"])
            files.append(str(out))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            from sustained_memory import main as analyze_main

            code = analyze_main([str(manifest), *files])
        result = json.loads(out.getvalue())
        self.assertEqual((code, result["status"]), (0, "passed"))
        self.assertEqual([f["path"] for f in result["observation_files"]], files)
        self.assertNotIn("observations_sha256", result)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(analyze_main([str(manifest), files[0]]), 2)  # n2 missing


class CollectCliTests(Harness):
    def setUp(self):
        super().setUp()
        self.fleet_ = self.fleet()
        self.plan = make_plan(self.fleet_)
        self.manifest = self.root / "manifest.json"
        self.manifest.write_text(json.dumps(self.plan))
        self.out = self.root / "cli.jsonl"

    def cli(self, *extra, nodes=NODES, stop_after=T0 + 30):
        args = ["collect", "--manifest", str(self.manifest), "--out", str(self.out)]
        for node in nodes:
            args += ["--node", f"{node}={self.fleet_.url(node)}"]
        stdout, stderr = io.StringIO(), io.StringIO()
        with (
            self.fleet_.readers(),
            contextlib.redirect_stdout(stdout),
            contextlib.redirect_stderr(stderr),
        ):
            code = csm.main(
                args + list(extra),
                now=self.clock.now,
                sleep=self.clock.sleep,
                fetch=self.fleet_.fetch,
                stopped=lambda: self.clock.now() > stop_after,
                paths=self.fleet_.paths,
            )
        return code, stdout.getvalue(), stderr.getvalue()

    def test_interrupted_cli_run_exits_incomplete_with_summary(self):
        code, stdout, _ = self.cli("--expected-network", "botho-testnet")
        self.assertEqual(code, 2)
        summary = json.loads(stdout)
        self.assertTrue(summary["interrupted"])
        self.assertEqual(
            summary["out_sha256"], hashlib.sha256(self.out.read_bytes()).hexdigest()
        )

    def test_full_cli_run_exits_zero(self):
        code, stdout, stderr = self.cli(stop_after=float("inf"))
        self.assertEqual(code, 0, stderr)
        self.assertTrue(json.loads(stdout)["complete"])

    def test_existing_output_is_never_reused(self):
        self.out.write_text("evidence\n")
        code, _, stderr = self.cli()
        self.assertEqual(code, 2)
        self.assertIn("exists", stderr)
        self.assertEqual(self.out.read_text(), "evidence\n")

    def test_preflight_identity_mismatch_writes_nothing(self):
        self.fleet_.exe["n2"] = b"other binary"
        code, _, stderr = self.cli()
        self.assertEqual(code, 2)
        self.assertIn("preflight identity mismatch", stderr)
        self.assertIn("artifact_sha256", stderr)
        self.assertFalse(self.out.exists())

    def test_configuration_refusals(self):
        for extra, nodes, why in (
            ((), ("n9",), "not a manifest node"),
            ((), ("n1", "n1"), "duplicate"),
            (("--rpc-timeout", "20"), NODES, "rpc-timeout"),
            (("--transfer-source", "n1=/x"), NODES, "payments"),
        ):
            with self.subTest(why=why):
                code, _, stderr = self.cli(*extra, nodes=nodes)
                self.assertEqual(code, 2)
                self.assertIn(why, stderr)
                self.assertFalse(self.out.exists())
        args = ["collect", "--manifest", str(self.manifest), "--out", str(self.out)]
        with self.fleet_.readers(), contextlib.redirect_stderr(io.StringIO()) as stderr:
            code = csm.main(
                args + ["--node", "n1=http://8.8.8.8/rpc"],
                now=self.clock.now,
                sleep=self.clock.sleep,
                fetch=self.fleet_.fetch,
                stopped=lambda: False,
                paths=self.fleet_.paths,
            )
        self.assertEqual(code, 2)
        self.assertIn("loopback", stderr.getvalue())

    def test_profile_already_over_or_t0_too_far(self):
        self.clock.time = T0 + 18031
        self.assertIn("already ended", self.cli()[2])
        self.clock.time = T0 - csm.MAX_LEAD_S - 1
        self.assertIn("ahead", self.cli()[2])
        self.assertFalse(self.out.exists())


if __name__ == "__main__":
    unittest.main()
