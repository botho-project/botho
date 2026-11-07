"""Failure-path tests use fake processes/API; real correspondence is separate."""

import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import nightly as n


FAKE = """#!/usr/bin/env python3
import json, pathlib, subprocess, sys, time
modes = json.loads(pathlib.Path(__file__).with_suffix('.json').read_text())
if '--list' in sys.argv:
    print('\\n'.join(modes)); sys.exit(0)
a = dict(zip(sys.argv[1::2], sys.argv[2::2]))
leg = a['--legs']; mode = modes[leg]
print('start', time.time(), flush=True)
if mode == 'timeout':
    subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(10)'])
    time.sleep(10)
if mode == 'model_failure': sys.exit(2)
path = pathlib.Path(a['--json'])
if mode == 'missing': sys.exit(0)
if mode == 'malformed': path.write_text('{'); sys.exit(0)
if mode == 'unwritable': path.mkdir(); sys.exit(0)
fixtures = pathlib.Path(a['--regressions']) / (leg + '.jsonl')
replayed = sum(bool(x.strip()) and not x.lstrip().startswith('#') for x in fixtures.read_text().splitlines())
w = {'input': {'n': 18446744073709551615}, 'lean': '0', 'rust': '1'}
fail = mode in ('mismatch', 'replayed', 'panic', 'lean_err')
if mode == 'replayed': w['replayed_fixture'] = str(fixtures) + ':1'
if mode == 'panic': w['rust'] = {'rust_panic': 'test panic'}
if mode == 'lean_err': w['lean'] = {'lean_err': 'bad request'}
if mode == 'mismatch':
    with fixtures.open('a') as f: f.write(json.dumps(w) + '\\n')
count = min(2, int(a['--cases'])) if fail else int(a['--cases'])
r = {'seed': a['--seed'], 'cases_per_leg': int(a['--cases']), 'legs': [
    {'leg': leg, 'rust': 'fixture::rust', 'cases': count, 'replayed': replayed,
     'seconds': 0.1, 'classes': {}, 'mismatches': [w] if fail else []}]}
if mode == 'wrong_seed': r['seed'] = '123'
if mode == 'wrong_leg': r['legs'][0]['leg'] = 'other'
if mode == 'short': r['legs'][0]['cases'] -= 1
if mode == 'short_replay': r['legs'][0]['replayed'] += 1
if mode == 'slow': time.sleep(0.08)
path.write_text(json.dumps(r))
print('end', time.time(), flush=True)
sys.exit(1 if fail or mode == 'inconsistent' else 0)
"""


class Helpers(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.out = self.root / "out"
        self.runner = self.root / "fake.py"
        self.runner.write_text(FAKE)
        self.runner.chmod(0o755)
        self.regressions = self.root / "committed"
        self.regressions.mkdir()
        self.args = SimpleNamespace(
            out=self.out,
            seed="18446744073709551615",
            cases=10,
            legs="",
            sha="a" * 40,
            repository="owner/repo",
            run_id="42",
            attempt="1",
            ref="refs/heads/test",
            runner=self.runner,
            model=self.root / "unused-model",
            regressions=self.regressions,
            workers=4,
            leg_timeout=1,
            total_timeout=5,
        )

    def execute(self, modes):
        self.runner.with_suffix(".json").write_text(json.dumps(modes))
        for leg in modes:
            (self.regressions / (leg + ".jsonl")).write_text(
                '# original\n{"input":{"n":7}}\n'
            )
        n.prepare(self.args)
        code = n.run(self.args)
        return code, n.inspect_bundle(self.out)

    def test_seed_extremes_and_rejection(self):
        for seed in ("0", str(2**64 - 1)):
            self.assertEqual(n.seed_value(seed), seed)
        with patch.object(n.secrets, "randbits", return_value=2**64 - 1):
            self.assertEqual(n.seed_value("random"), str(2**64 - 1))
        for seed in ("-1", "18446744073709551616", "1.0", "", "$(echo nope)"):
            with self.assertRaises(ValueError):
                n.seed_value(seed)

    def test_dynamic_discovery_and_full_clean_coverage(self):
        code, (meta, findings, rows) = self.execute(
            {"new_leg": "clean", "another": "clean"}
        )
        self.assertEqual(code, 0)
        self.assertEqual(meta["selected_legs"], ["new_leg", "another"])
        self.assertEqual(meta["seed"], str(2**64 - 1))
        self.assertEqual(findings, {})
        self.assertTrue(all("generated 10/10, replayed 1" in row for row in rows))
        self.assertEqual(
            (self.regressions / "new_leg.jsonl").read_text(),
            '# original\n{"input":{"n":7}}\n',
        )

    def test_discovery_rejects_empty_duplicate_and_unsafe_names(self):
        for output in ("", "a\na\n", "../escape\n", "good\n\n"):
            with patch.object(
                n.subprocess, "run", return_value=SimpleNamespace(stdout=output)
            ):
                with self.assertRaises(ValueError):
                    n.discover(self.runner)
        for text in ("bad", "a,a", ","):
            with self.assertRaises(ValueError):
                n.selection(text, ["a"])

    def test_subset_and_seed_zero(self):
        self.args.legs, self.args.seed = "second", "0"
        code, (meta, _, _) = self.execute({"first": "model_failure", "second": "clean"})
        self.assertEqual(code, 0)
        self.assertEqual(meta["selected_legs"], ["second"])
        self.assertEqual(meta["seed"], "0")

    def test_early_stop_replayed_and_panic_fixtures(self):
        code, (_, findings, rows) = self.execute(
            {"generated": "mismatch", "stored": "replayed", "panic": "panic"}
        )
        self.assertEqual(code, 1)
        self.assertEqual(len(findings), 3)
        self.assertTrue(all("generated 2/10" in row for row in rows))
        self.assertEqual((self.out / "appended/stored.jsonl").read_bytes(), b"")
        for leg in ("generated", "stored", "panic"):
            self.assertTrue((self.out / "failing" / (leg + ".jsonl")).is_file())
        self.assertIn('"input"', (self.out / "appended/generated.jsonl").read_text())

    def test_infrastructure_does_not_hide_independent_results(self):
        modes = {
            x: x
            for x in (
                "missing",
                "malformed",
                "unwritable",
                "wrong_seed",
                "wrong_leg",
                "short",
                "short_replay",
                "inconsistent",
                "lean_err",
                "model_failure",
            )
        }
        modes.update(good="clean", confirmed="mismatch")
        code, (_, findings, rows) = self.execute(modes)
        self.assertEqual(code, 1)
        self.assertEqual(len(rows), len(modes))
        self.assertEqual(len(findings), 1)
        self.assertEqual(sum(": infrastructure" in row for row in rows), 10)
        self.assertTrue(any("good: clean" in row for row in rows))

    def test_timeout_collects_other_leg(self):
        self.args.leg_timeout = 0.2
        start = time.monotonic()
        code, (_, findings, rows) = self.execute({"hung": "timeout", "good": "clean"})
        self.assertLess(time.monotonic() - start, 3)
        self.assertEqual(code, 1)
        self.assertFalse(findings)
        self.assertTrue(any("good: clean" in row for row in rows))
        self.assertTrue(n.read_json(self.out / "legs/hung/status.json")["timeout"])

    def test_worker_limit_and_queue_deadline_are_explicit(self):
        self.args.workers = 2
        code, (_, _, _) = self.execute({f"leg_{i}": "slow" for i in range(7)})
        self.assertEqual(code, 0)
        events = []
        for log in (self.out / "legs").glob("*/process.log"):
            for line in log.read_text().splitlines():
                action, timestamp = line.split()
                events.append((float(timestamp), 1 if action == "start" else -1))
        active, maximum = 0, 0
        for _, change in sorted(events):
            active += change
            maximum = max(maximum, active)
        self.assertLessEqual(maximum, 2)
        self.args.workers = 5
        with self.assertRaises(ValueError):
            n.run(self.args)

    def test_expired_queue_is_recorded_and_never_reported_clean(self):
        self.args.workers = 1
        self.args.total_timeout = 0.04
        code, (_, findings, rows) = self.execute({"first": "slow", "queued": "clean"})
        self.assertEqual(code, 1)
        self.assertFalse(findings)
        self.assertEqual(len(rows), 2)
        collection = n.read_json(self.out / "collection.json")
        self.assertIn("deadline", collection["queued"]["error"])

    def test_missing_leg_collection_rejected(self):
        self.execute({"first": "clean", "second": "clean"})
        n.write_json(self.out / "collection.json", {"first": {}})
        with self.assertRaisesRegex(ValueError, "missing-leg"):
            n.inspect_bundle(self.out)

    def test_malformed_status_does_not_discard_other_findings(self):
        self.execute({"bad": "clean", "good": "mismatch"})
        collection = n.read_json(self.out / "collection.json")
        collection["bad"] = []
        n.write_json(self.out / "collection.json", collection)
        n.write_json(self.out / "legs/bad/status.json", [])
        _, findings, rows = n.inspect_bundle(self.out)
        self.assertEqual(len(findings), 1)
        self.assertTrue(any("bad: infrastructure" in row for row in rows))

    def test_report_schema_and_count_types_fail_closed(self):
        self.execute({"leg": "clean"})
        report = n.read_json(self.out / "legs/leg/report.json")
        meta = n.read_json(self.out / "metadata.json")
        status = {"exit": 0, "timeout": False}
        variants = []
        for field in ("cases", "replayed"):
            r = copy.deepcopy(report)
            r["legs"][0][field] = True
            variants.append(r)
        r = copy.deepcopy(report)
        r["cases_per_leg"] = True
        variants.append(r)
        r = copy.deepcopy(report)
        r["seed"] = 18446744073709551615
        variants.append(r)
        r = copy.deepcopy(report)
        r["legs"].append(r["legs"][0])
        variants.append(r)
        for r in variants:
            with self.assertRaises(ValueError):
                n.validate_report(r, status, meta, "leg")
        with self.assertRaises(ValueError):
            n.validate_report(report, {"exit": True, "timeout": False}, meta, "leg")
        for raw in ('{"seed":1,"seed":2}', '{"x":NaN}'):
            path = self.root / "invalid.json"
            path.write_text(raw)
            with self.assertRaises(ValueError):
                n.read_json(path)

    def test_lottery_sim_named_envelope_and_replay_counts(self):
        self.execute({"lottery_sim": "clean"})
        report = n.read_json(self.out / "legs/lottery_sim/report.json")
        meta = n.read_json(self.out / "metadata.json")
        clean = {"node": [{"paid": "0"}], "simulator": [{"paid": "0"}],
                 "simulator_state": {"ok": True, "failed_checks": []}}
        changed = copy.deepcopy(clean)
        changed["simulator_state"] = {"ok": False, "failed_checks": ["0/total_distributed"]}
        row = report["legs"][0]
        row["classes"] = {"sequence/one/state_failed": row["cases"]}
        row["mismatches"] = [{"input": {"carryover": str(2**128 - 1)},
                              "lean": clean, "rust": changed}]
        self.assertEqual(n.validate_report(report, {"exit": 1, "timeout": False}, meta,
                                          "lottery_sim"), "mismatch")
        row["mismatches"][0]["rust"] = copy.deepcopy(clean)
        with self.assertRaisesRegex(ValueError, "equal witness"):
            n.validate_report(report, {"exit": 1, "timeout": False}, meta, "lottery_sim")
        row["mismatches"] = []
        self.assertEqual(n.validate_report(report, {"exit": 0, "timeout": False}, meta,
                                          "lottery_sim"), "clean")
        row["cases"] -= 1
        with self.assertRaises(ValueError):
            n.validate_report(report, {"exit": 0, "timeout": False}, meta, "lottery_sim")
        meta["cases"] = report["cases_per_leg"] = row["cases"] = 0
        row["classes"] = {}
        self.assertEqual(n.validate_report(report, {"exit": 0, "timeout": False}, meta,
                                          "lottery_sim"), "clean")
        row["mismatches"] = [{"input": {}, "lean": clean, "rust": changed,
                              "replayed_fixture": "lottery_sim.jsonl:1"}]
        self.assertEqual(n.validate_report(report, {"exit": 1, "timeout": False}, meta,
                                          "lottery_sim"), "mismatch")
        row["replayed"] += 1
        with self.assertRaisesRegex(ValueError, "replay count"):
            n.validate_report(report, {"exit": 1, "timeout": False}, meta, "lottery_sim")

    def test_equal_witness_and_mixed_lean_error_reject_entire_leg(self):
        self.execute({"leg": "mismatch"})
        report = n.read_json(self.out / "legs/leg/report.json")
        meta = n.read_json(self.out / "metadata.json")
        witness = report["legs"][0]["mismatches"][0]
        for lean, rust in [(1, 1), ({"lean_err": "bad"}, 1)]:
            bad = copy.deepcopy(report)
            bad["legs"][0]["mismatches"].append(dict(witness, lean=lean, rust=rust))
            with self.assertRaises(ValueError):
                n.validate_report(bad, {"exit": 1, "timeout": False}, meta, "leg")

    def test_unwritable_status_still_collects_other_leg(self):
        real = n.write_json

        def write(path, value):
            if path.parts[-3:] == ("legs", "bad", "status.json"):
                raise OSError("test permission denied")
            return real(path, value)

        with patch.object(n, "write_json", side_effect=write):
            code, (_, _, rows) = self.execute({"bad": "clean", "good": "clean"})
        self.assertEqual(code, 1)
        self.assertTrue(any("good: clean" in row for row in rows))

    def test_report_does_not_follow_artifact_directory_symlinks(self):
        self.out.mkdir()
        external = self.root / "external"
        external.mkdir()
        n.write_json(external / "report.json", {"secret": "not artifact data"})
        (self.out / "link").symlink_to(external, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "escapes"):
            n.bundle_json(self.out, "link", "report.json")

    def test_report_binds_trusted_context_before_api(self):
        self.execute({"leg": "mismatch"})
        self.args.sha = "b" * 40
        self.args.summary = self.root / "summary.md"
        self.args.artifact_url = (
            "https://github.com/owner/repo/actions/runs/42/artifacts/123"
        )
        with patch.object(n, "GitHub") as api:
            with self.assertRaisesRegex(ValueError, "sha differs"):
                n.report(self.args)
            api.assert_not_called()


class FakeAPI:
    def __init__(self, issues=()):
        self.stored = list(issues)
        self.calls = []
        self.fail = False

    def issues(self):
        yield from self.stored

    def request(self, method, endpoint, body):
        self.calls.append((method, endpoint, body))
        if self.fail:
            raise subprocess.CalledProcessError(1, "gh")
        if method == "POST":
            issue = dict(
                body,
                number=len(self.stored) + 1,
                state="open",
                user={"login": "github-actions[bot]"},
                html_url="https://github.com/o/r/issues/1",
            )
            self.stored.append(issue)
        else:
            issue = self.stored[0]
            issue.update(body)
        return issue


class Reporting(unittest.TestCase):
    def setUp(self):
        self.meta = {
            "seed": str(2**64 - 1),
            "sha": "a" * 40,
            "run_id": "9",
            "attempt": "1",
            "ref": "refs/heads/test",
            "cases": 100,
            "run_url": "https://github.com/o/r/actions/runs/9",
        }
        self.witness = {
            "input": {"z": [2, 1], "a": 2**64 - 1},
            "lean": "0",
            "rust": "1",
        }
        self.digest = n.fingerprint("leg", self.witness["input"])
        self.findings = {self.digest: ("leg", self.witness, 10)}
        self.artifact = self.meta["run_url"] + "/artifacts/22"

    def test_fingerprint_exact_stability_and_distinctions(self):
        self.assertEqual(
            self.digest, n.fingerprint("leg", {"a": 2**64 - 1, "z": [2, 1]})
        )
        for leg, value in [
            ("other", self.witness["input"]),
            ("leg", {"z": [1, 2], "a": 2**64 - 1}),
            ("leg", {"z": [2, 1], "a": str(2**64 - 1)}),
        ]:
            self.assertNotEqual(self.digest, n.fingerprint(leg, value))
        with self.assertRaises(ValueError):
            n.fingerprint("leg", {"float": 1.0})

    def test_create_same_attempt_noop_recurrence_update_and_reopen(self):
        api = FakeAPI()
        n.publish(self.meta, self.findings, api, self.artifact)
        self.assertEqual(api.calls[0][2]["labels"], ["area:testing", "loom:triage"])
        n.publish(self.meta, self.findings, api, self.artifact)
        self.assertEqual(len(api.calls), 1)
        api.stored[0]["state"] = "closed"
        self.meta.update(seed="0", attempt="2")
        n.publish(self.meta, self.findings, api, self.artifact)
        self.assertEqual(len(api.calls), 2)
        self.assertEqual(api.calls[-1][0], "PATCH")
        self.assertEqual(api.stored[0]["state"], "open")
        self.assertIn("Seed: `0`", api.stored[0]["body"])

    def test_api_paginates_all_states_and_exact_markers(self):
        api = n.GitHub("o/r")
        stored = FakeAPI()
        n.publish(self.meta, self.findings, stored, self.artifact)
        self.meta["attempt"] = "2"
        pages = [
            [{"number": x, "body": "unrelated"} for x in range(100)],
            stored.stored,
        ]
        with patch.object(api, "request", side_effect=pages) as call:
            issues = list(api.issues())
        self.assertEqual(len(issues), 101)
        self.assertIn("state=all&per_page=100&page=2", call.call_args.args[1])
        fake = FakeAPI(issues)
        # Exact matching ignores an unrelated human issue and a malformed marker.
        fake.stored = [
            dict(
                stored.stored[0],
                body=stored.stored[0]["body"].replace(self.digest, self.digest + "0"),
            )
        ]
        n.publish(self.meta, self.findings, fake, self.artifact)
        self.assertEqual(fake.calls[-1][0], "POST")

    def test_exact_marker_survives_prose_and_label_edits(self):
        api = FakeAPI()
        n.publish(self.meta, self.findings, api, self.artifact)
        api.stored[0]["body"] = api.stored[0]["body"].replace(
            "## " + n.TITLE, "Human triage notes"
        )
        api.stored[0]["labels"] = []
        self.meta["attempt"] = "2"
        n.publish(self.meta, self.findings, api, self.artifact)
        self.assertEqual(api.calls[-1][0], "PATCH")
        self.assertEqual(len(api.stored), 1)

    def test_api_failure_is_not_retried_or_greenwashed(self):
        api = FakeAPI()
        api.fail = True
        with self.assertRaises(subprocess.CalledProcessError):
            n.publish(self.meta, self.findings, api, self.artifact)
        self.assertEqual(len(api.calls), 1)
        self.assertEqual(api.stored, [])
        api.fail = False
        n.publish(self.meta, self.findings, api, self.artifact)
        self.meta["attempt"] = "2"
        api.fail = True
        with self.assertRaises(subprocess.CalledProcessError):
            n.publish(self.meta, self.findings, api, self.artifact)
        self.assertEqual(api.calls[-1][0], "PATCH")
        self.assertEqual(len(api.stored), 1)

    def test_lookup_failure_prevents_creation(self):
        api = n.GitHub("o/r")
        with patch.object(
            api, "request", side_effect=subprocess.CalledProcessError(1, "gh")
        ) as call:
            with self.assertRaises(subprocess.CalledProcessError):
                n.publish(self.meta, self.findings, api, self.artifact)
        self.assertEqual(call.call_count, 1)
        self.assertEqual(call.call_args.args[0], "GET")

    def test_existing_issue_on_second_page_updates_without_create(self):
        saved = FakeAPI()
        n.publish(self.meta, self.findings, saved, self.artifact)
        self.meta["attempt"] = "2"
        api = n.GitHub("o/r")
        pages = [
            [{"number": x, "body": "other"} for x in range(100)],
            saved.stored,
            saved.stored[0],
        ]
        with patch.object(api, "request", side_effect=pages) as call:
            n.publish(self.meta, self.findings, api, self.artifact)
        self.assertEqual(
            [c.args[0] for c in call.call_args_list], ["GET", "GET", "PATCH"]
        )

    def test_caught_panic_and_bounded_escaped_fixture(self):
        self.witness["rust"] = {"rust_panic": "<script>" + "&" * 70_000}
        body = n.issue_body(
            self.meta, "leg", self.witness, 10, self.digest, self.artifact
        )
        self.assertIn("Caught Rust panic", body)
        self.assertIn("TRUNCATED", body)
        self.assertIn("failing/leg.jsonl", body)
        self.assertNotIn("<script>", body)
        self.assertLess(len(body.encode()), 60_000)


class Workflow(unittest.TestCase):
    def execute_build_step(self, name, builder, exit_code):
        """Run the real YAML script under Actions' implicit bash -e shell."""
        root = Path(__file__).resolve().parents[2]
        workflow = (root / ".github/workflows/formal-drt-nightly.yml").read_text()
        step = workflow.split(f"      - name: {name}\n", 1)[1].split("\n      - ", 1)[0]
        script = textwrap.dedent(step.split("        run: |\n", 1)[1])
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            lean = scratch / "formal-drt/lean"
            lean.mkdir(parents=True)
            (lean / "lean-toolchain").write_text("leanprover/lean4:v4.35.0\n")
            (scratch / "Cargo.lock").write_text("test lockfile\n")
            diagnostics = scratch / "nightly"
            diagnostics.mkdir()
            metadata = diagnostics / "metadata.json"
            metadata.write_text('{"seed":"0"}\n')
            fake_bin = scratch / "fake-bin"
            fake_bin.mkdir()
            for tool in ("elan", "lake", "cargo"):
                stub = fake_bin / tool
                stub.write_text(
                    f"#!/bin/sh\nprintf '{tool} build diagnostic\\n' >&2\n"
                    f"exit {exit_code if tool == builder else 0}\n"
                )
                stub.chmod(0o755)
            # Cache restoration happened before the failed build. Running this
            # executable would falsely attribute stale code to the current SHA.
            cached = scratch / "formal-drt/target/release/botho-formal-drt"
            cached.parent.mkdir(parents=True)
            cached.write_text('#!/bin/sh\ntouch "$DRT_SENTINEL"\n')
            cached.chmod(0o755)
            sentinel = scratch / "drt-executed"
            env = dict(
                os.environ,
                PATH=str(fake_bin) + os.pathsep + os.environ["PATH"],
                RUNNER_TEMP=str(scratch),
                DRT_SENTINEL=str(sentinel),
            )
            for option in ("BASH_ENV", "SHELLOPTS", "BASHOPTS"):
                env.pop(option, None)
            script_path = scratch / "build-step.sh"
            script_path.write_text(script)
            result = subprocess.run(
                ["bash", "-e", str(script_path)],
                env=env,
                cwd=lean if "working-directory: formal-drt/lean" in step else scratch,
                capture_output=True,
                text=True,
                timeout=10,
            )
            # GitHub's next ordinary step has an implicit success() condition.
            # Exercise the cached executable if the build incorrectly goes green.
            if result.returncode == 0:
                subprocess.run([str(cached)], env=env, check=True, timeout=5)
            log = diagnostics / (
                "build-lean.log" if builder == "lake" else "build-rust.log"
            )
            self.assertEqual(log.read_text(), f"{builder} build diagnostic\n")
            self.assertEqual(metadata.read_text(), '{"seed":"0"}\n')
            self.assertTrue(cached.is_file())
            self.assertEqual(
                result.returncode,
                exit_code,
                f"{name}: cached DRT executed={sentinel.exists()}",
            )
            self.assertEqual(sentinel.exists(), exit_code == 0)

    def test_model_build_failure_skips_cached_drt_and_retains_log(self):
        self.execute_build_step("Build pinned model", "lake", 37)

    def test_runner_build_failure_skips_cached_drt_and_retains_log(self):
        self.execute_build_step("Build pinned runner", "cargo", 42)

    def test_successful_build_steps_allow_downstream_drt(self):
        for name, builder in (
            ("Build pinned model", "lake"),
            ("Build pinned runner", "cargo"),
        ):
            with self.subTest(builder=builder):
                self.execute_build_step(name, builder, 0)

    def test_permissions_guards_bounds_and_pr_defaults(self):
        root = Path(__file__).resolve().parents[2]
        nightly = (root / ".github/workflows/formal-drt-nightly.yml").read_text()
        pr = (root / ".github/workflows/formal-drt.yml").read_text()
        self.assertIn("cron: '23 3 * * *'", nightly)
        self.assertIn("group: formal-drt-nightly\n  cancel-in-progress: false", nightly)
        self.assertNotIn("pull_request_target", nightly)
        run, report = nightly.split("\n  report:\n")
        self.assertNotIn("issues: write", run)
        self.assertNotIn("GH_TOKEN", run)
        self.assertIn("issues: write", report)
        self.assertIn("ref: ${{ github.event.repository.default_branch }}", report)
        for text in (run, report):
            self.assertIn("github.repository == 'botho-project/botho'", text)
            self.assertIn("github.event_name == 'workflow_dispatch'", text)
        self.assertIn("--workers 4", run)
        self.assertIn("--leg-timeout 3600 --total-timeout 9600", run)
        self.assertIn("timeout-minutes: 240", run)
        self.assertIn("timeout-minutes: 10", report)
        self.assertIn("retention-days: 14", run)
        for name in (
            "Upload diagnostics on clean or failed execution",
            "Summarize coverage and artifact",
        ):
            step = run.split(f"      - name: {name}\n", 1)[1].split("\n      - ", 1)[0]
            self.assertIn("        if: always()", step)
        self.assertIn("github.event.inputs.cases || '10000'", pr)
        self.assertIn("github.event.inputs.seed || '1707'", pr)
        self.assertNotIn("issues: write", pr)
        self.assertIn("python3 -m unittest discover -s formal-drt/scripts", pr)


if __name__ == "__main__":
    unittest.main()
