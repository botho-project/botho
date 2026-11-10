"""Lottery labels are validated independently of snapshots and before writes."""
import contextlib
import io
import json
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import check_research_tables as tables


class MetadataTests(unittest.TestCase):
    def setUp(self):
        self.generators = [g for g in tables.GENERATORS if g.name.startswith("lottery-")]

    def output(self, gen):
        return "\n".join(f"Research-{k}: {v}" for k, v in tables.lottery_metadata(gen).items()) + "\n\n| cell |\n| 1913 |\n"

    def test_all_four_valid_and_all_twenty_five_generators_preserved(self):
        self.assertEqual(len(tables.GENERATORS), 25)
        self.assertEqual(len(self.generators), 4)
        self.assertEqual({tables.validate_metadata(g, self.output(g))["Surface"] for g in self.generators},
                         set(tables.LOTTERY_METADATA))

    def test_missing_duplicate_unknown_and_swapped_metadata(self):
        for gen in self.generators:
            good = self.output(gen)
            expected = tables.lottery_metadata(gen)
            for key in expected:
                bad = "\n".join(l for l in good.splitlines() if not l.startswith(f"Research-{key}:"))
                with self.subTest(gen=gen.name, missing=key), self.assertRaises(ValueError):
                    tables.validate_metadata(gen, bad)
            wrong = {
                "Surface": ["unknown", *[k for k in tables.LOTTERY_METADATA if k != expected["Surface"]]],
                "Command": ["lottery-reward-cap-sweep --faithful", "lottery-reward-cap-sweep", "unknown"],
                "Classification": ["production", "external_assumption"],
                "RNG": ["production V2 draw", "none", tables.LOTTERY_METADATA["sweep.reward_cap.faithful"]["RNG"]],
                "Method": ["production payout simulation"],
                "Fees": ["800-permille production annual track", "hypothetical charge is consensus floor"],
            }
            for key, variants in wrong.items():
                for value in variants:
                    if value == expected[key]:
                        continue
                    bad = good.replace(f"Research-{key}: {expected[key]}", f"Research-{key}: {value}")
                    with self.subTest(gen=gen.name, field=key, value=value), self.assertRaises(ValueError):
                        tables.validate_metadata(gen, bad)
            for extra in ["Research-Classification: research", "Research-Waiver: production", "Research-Surface"]:
                with self.assertRaises(ValueError):
                    tables.validate_metadata(gen, good + extra)
        with self.assertRaises(ValueError):
            tables.lottery_metadata(tables.Generator("lottery-selection-sweep", ("unknown",), "", False))

    def invoke(self, gen, output, golden, update=False):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            binary = directory / "synthetic-cli"
            binary.write_text(f"#!{sys.executable}\nimport sys\nsys.stdout.write({output!r})\n")
            binary.chmod(0o755)
            gold = directory / f"{gen.name}.txt"
            gold.write_text(golden)
            gen = tables.Generator(gen.name, gen.args, gen.doc, False)
            with patch.object(tables, "GENERATORS", [gen]), patch.object(tables, "GOLDEN_DIR", directory), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                rc = tables.main(["--bin", str(binary), *(["--update"] if update else [])])
            return rc, gold.read_text()

    def test_equal_bad_output_and_golden_is_rejected(self):
        gen = self.generators[0]
        bad = self.output(gen).replace("Research-Classification: research", "Research-Classification: production")
        self.assertEqual(self.invoke(gen, bad, bad)[0], 1)

    def test_bad_output_cannot_replace_golden(self):
        gen = self.generators[0]
        good = self.output(gen)
        bad = good.replace("Research-Surface:", "Removed-Surface:")
        self.assertEqual(self.invoke(gen, bad, good, update=True), (1, good))

    def test_valid_update_and_check_and_unrelated_generator(self):
        gen = self.generators[0]
        good = self.output(gen)
        self.assertEqual(self.invoke(gen, good, "old", update=True), (0, good))
        self.assertEqual(self.invoke(gen, good, good), (0, good))
        unrelated = tables.Generator("other", ("other",), "", False)
        self.assertEqual(self.invoke(unrelated, "unrelated\n", "unrelated\n"), (0, "unrelated\n"))

    def test_table_extractor_ignores_only_non_table_metadata(self):
        gen = self.generators[0]
        self.assertEqual(tables.table_blocks(self.output(gen)), [["| cell |", "| 1913 |"]])


class BuildReceiptTests(unittest.TestCase):
    def test_actual_subprocess_artifact_hash_and_rejections(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            binary = directory / "binary"
            binary.write_bytes(b"synthetic test artifact")
            event = {"reason": "compiler-artifact", "target": {"name": "test-target"}, "executable": str(binary)}
            command = [sys.executable, "-c", f"print({json.dumps(event)!r})"]
            receipt = directory / "build.json"
            context = {"head": "source", "tree": "tree", "source_sha256": {"s": "hash"}, "profile_environment": {}, "toolchain": {"cargo": "synthetic", "rustc": "synthetic"}}
            with patch.object(tables, "execution_context", return_value=context):
                tables.capture_cargo_build(command, "test-target", receipt)
                record = tables.verified_build(receipt, binary)
                self.assertEqual(record["command"], command)
                self.assertEqual(record["exit"], 0)
                self.assertEqual(record["binary_sha256"], tables.digest(binary.read_bytes()))
                for field in ("head", "tree", "source_sha256", "profile_environment", "toolchain"):
                    changed = {**context, field: "changed"}
                    with patch.object(tables, "execution_context", return_value=changed), self.assertRaises(ValueError):
                        tables.verified_build(receipt, binary)
                binary.write_bytes(b"unrelated artifact")
                with self.assertRaises(ValueError):
                    tables.verified_build(receipt, binary)
                with self.assertRaises(RuntimeError):
                    tables.capture_cargo_build([sys.executable, "-c", "raise SystemExit(7)"], "test-target", receipt)
                self.assertEqual(json.loads(receipt.read_text())["exit"], 7)
                with self.assertRaises(RuntimeError):
                    tables.capture_cargo_build(command, "missing-target", receipt)

    def test_parity_requires_a_fresh_payload_even_after_successful_command(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            binary = directory / "binary"
            binary.write_bytes(b"synthetic test executable")
            build = directory / "build.json"
            build.write_text(json.dumps({"binary": str(binary)}))
            payload = directory / "production-lottery.json"
            payload.write_text('{"stale": true}')
            completed = subprocess.CompletedProcess([], 0, b"test command succeeded\n", b"")
            with patch.object(tables, "verified_build"), patch.object(tables, "execution_context", return_value={}), \
                    patch.object(tables.subprocess, "run", return_value=completed) as command, \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaisesRegex(RuntimeError, "payload missing"):
                    tables.capture_parity(build, directory)
                command.assert_called_once()
            self.assertFalse(payload.exists())
            receipt = json.loads((directory / "parity-receipt.json").read_text())
            self.assertEqual(receipt["exit"], 0)  # command success is insufficient
            self.assertFalse(receipt["payload_produced"])
            self.assertIsNone(receipt["payload_sha256"])

    def test_workflow_update_preserves_committed_drift_guard(self):
        workflow = (tables.REPO / ".github/workflows/research-sims.yml").read_text()
        self.assertIn("git diff --exit-code -- docs/research/generated/", workflow)
        self.assertGreater(workflow.index("git diff --exit-code"), workflow.index("--update"))
        self.assertIn("if: always()", workflow)
        parity = (tables.REPO / ".github/workflows/workspace-build.yml").read_text()
        self.assertIn("capture_parity(receipt, evidence)", parity)
        self.assertIn('"--no-run", "--message-format=json-render-diagnostics"', parity)


if __name__ == "__main__":
    unittest.main()
