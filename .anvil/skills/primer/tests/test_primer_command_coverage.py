"""Structural smoke tests for the ``anvil:primer`` skill (issue #686).

These tests assert **structural properties** of the shipped skill files
(files exist, frontmatter parses, the rubric declares 9 dimensions summing
to 44 with a >=35 advance threshold under the ``anvil-primer-v1`` id,
pedagogical scaffolding is the OWNED dominant dim 1 at weight 7, the two
spec-consistency critical flags + the technical-accuracy critical flag are
documented, every critic-writing command stamps the #346 rubric fields and
uses the staged-sidecar primitive, the ``spec_ref`` activation contract is
documented, and the report-shaped audit+figures lifecycle is present). They
are intentionally NOT golden-file tests — the skill is a generative
authoring skill and prose varies across runs and models.

Runs under either ``pytest anvil/skills/primer/tests/`` or
``python -m unittest discover anvil/skills/primer/tests/``.

Per the #58 packaging convention this filename
(``test_primer_command_coverage``) is unique across the
``anvil/skills/*/tests/`` tree; the package carries an ``__init__.py`` to
avoid the cross-skill pytest collection collision.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

_SKILL_ROOT = Path(__file__).resolve().parent.parent
_REPO_ROOT = _SKILL_ROOT.parents[2]

RUBRIC_ID = "anvil-primer-v1"

# Every critic-writing command must carry the #346 stamps + the atomic
# sidecar primitive.
CRITIC_COMMANDS = ("commands/primer-review.md", "commands/primer-audit.md")


def _read(rel: str) -> str:
    return (_SKILL_ROOT / rel).read_text(encoding="utf-8")


def _parse_frontmatter(text: str) -> dict:
    """Parse a leading ``---``-delimited YAML frontmatter block.

    Uses PyYAML when available; falls back to a minimal ``key: value``
    parser so the test does not hard-depend on PyYAML being installed.
    """
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}
    end = None
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            end = i
            break
    if end is None:
        return {}
    block = "\n".join(lines[1:end])
    try:
        import yaml  # type: ignore

        data = yaml.safe_load(block)
        return data if isinstance(data, dict) else {}
    except Exception:
        result: dict = {}
        for line in block.splitlines():
            line = line.strip()
            if not line or line.startswith("#") or ":" not in line:
                continue
            key, _, value = line.partition(":")
            result[key.strip()] = value.strip().strip('"').strip("'")
        return result


class TestFilesExist(unittest.TestCase):
    """The pinned file manifest is present on disk (v1 scope)."""

    EXPECTED = [
        "SKILL.md",
        "rubric.md",
        "README.md",
        "__init__.py",
        "commands/primer.md",
        "commands/primer-draft.md",
        "commands/primer-review.md",
        "commands/primer-audit.md",
        "commands/primer-revise.md",
        "commands/primer-figures.md",
        "templates/BRIEF.md.example",
        "templates/primer.template.md",
        "tests/__init__.py",
        "tests/test_primer_command_coverage.py",
    ]

    def test_manifest_present(self):
        for rel in self.EXPECTED:
            with self.subTest(path=rel):
                self.assertTrue(
                    (_SKILL_ROOT / rel).exists(), f"missing skill file: {rel}"
                )

    def test_deferred_scope_absent(self):
        # v1 defers the worked example, voice wiring, figure-adapter
        # registry, and LaTeX/TikZ path (issue #686 curation).
        self.assertFalse(
            (_SKILL_ROOT / "examples").exists(),
            "examples/ is deferred scope (the Botho #881 worked example)",
        )
        # No voice-wiring or figure-adapter commands leaked in.
        for stem in ("primer-voice", "primer-figure-adapter"):
            with self.subTest(command=stem):
                self.assertFalse(
                    (_SKILL_ROOT / "commands" / f"{stem}.md").exists(),
                    f"{stem}.md is deferred scope (issue #686 curation)",
                )


class TestSkillFrontmatter(unittest.TestCase):
    """SKILL.md frontmatter matches the sibling skills' shape."""

    def test_frontmatter(self):
        fm = _parse_frontmatter(_read("SKILL.md"))
        self.assertEqual(fm.get("name"), "primer")
        self.assertEqual(fm.get("domain"), "primer")
        self.assertEqual(fm.get("type"), "skill")
        self.assertIn(fm.get("user-invocable"), (False, "false"))

    def test_report_shaped_lifecycle(self):
        text = _read("SKILL.md")
        # Borrows the report lifecycle shape (draft → parallel review+audit
        # → revise → AUDITED, plus figures) — NOT a report parameterization.
        self.assertIn("Relationship to `anvil:report`", text)
        self.assertIn("skill identity = artifact identity", text)
        self.assertIn("REVIEWED+AUDITED", text)
        self.assertIn("AUDITED", text)

    def test_spec_ref_activation_contract_documented(self):
        text = _read("SKILL.md")
        self.assertIn("Spec-ref contract", text)
        self.assertIn("resolve_spec_ref", text)
        # Declared-but-missing is a defect to surface (major), absent is off.
        self.assertIn("major", text)
        self.assertIn("silent", text.lower())

    def test_deferred_section_names_the_deferrals(self):
        text = _read("SKILL.md").lower()
        self.assertIn("deferred", text)
        for token in ("worked example", "voice", "figure-adapter", "tikz"):
            with self.subTest(token=token):
                self.assertIn(token, text)

    def test_sidecar_stamping_and_scorecard_contracts_referenced(self):
        text = _read("SKILL.md")
        self.assertIn("staged_sidecar", text)
        self.assertIn(RUBRIC_ID, text)
        self.assertIn("human-verdict", text)


class TestCommandFrontmatter(unittest.TestCase):
    """Every command file carries a name/description frontmatter block."""

    COMMANDS = {
        "commands/primer.md": "primer",
        "commands/primer-draft.md": "primer-draft",
        "commands/primer-review.md": "primer-review",
        "commands/primer-audit.md": "primer-audit",
        "commands/primer-revise.md": "primer-revise",
        "commands/primer-figures.md": "primer-figures",
    }

    def test_command_frontmatter(self):
        for rel, expected_name in self.COMMANDS.items():
            with self.subTest(path=rel):
                fm = _parse_frontmatter(_read(rel))
                self.assertEqual(fm.get("name"), expected_name)
                self.assertTrue(
                    fm.get("description"), f"{rel} missing a description"
                )


class TestCriticCommandStamping(unittest.TestCase):
    """Both critic-writing commands stamp #346 fields + use staged_sidecar."""

    def test_stamps_and_sidecar_in_every_critic(self):
        for rel in CRITIC_COMMANDS:
            with self.subTest(command=rel):
                text = _read(rel)
                self.assertIn(RUBRIC_ID, text)
                self.assertIn("rubric_total: 44", text)
                self.assertIn("advance_threshold: 35", text)
                self.assertIn("staged_sidecar", text)
                self.assertIn("cleanup_one_staging", text)
                self.assertIn("human-verdict", text)


class TestSpecRefContractInCommands(unittest.TestCase):
    """The spec_ref activation contract is wired into draft/review/audit."""

    def test_audit_resolves_spec_ref_and_sweeps_when_present(self):
        text = _read("commands/primer-audit.md")
        self.assertIn("resolve_spec_ref", text)
        self.assertIn("Contradicts cited spec", text)
        # Absent → major finding, not a crash, not a false flag.
        self.assertIn("major", text)
        self.assertIn("cannot fire", text)
        # Unresolvable path → graceful degradation, never raises.
        self.assertIn("never raises", text)
        self.assertIn("no false critical flag", text)

    def test_review_raises_duplication_flag_when_spec_ref_active(self):
        text = _read("commands/primer-review.md")
        self.assertIn("resolve_spec_ref", text)
        self.assertIn("Duplicates formal spec section", text)
        self.assertIn("cannot fire", text)

    def test_draft_resolves_optional_spec_ref(self):
        text = _read("commands/primer-draft.md")
        self.assertIn("resolve_spec_ref", text)
        self.assertIn("spec_ref_resolved", text)


class TestRubric(unittest.TestCase):
    """rubric.md declares 9 dims summing to 44, >=35, pedagogy-dominant."""

    def setUp(self):
        self.text = _read("rubric.md")

    def test_nine_dimensions_sum_to_forty_four(self):
        rows = re.findall(
            r"^\|\s*([1-9])\s*\|\s*\*\*[^|]+\*\*\s*\|\s*(\d+)\s*\|",
            self.text,
            flags=re.MULTILINE,
        )
        self.assertEqual(
            len(rows), 9, f"expected 9 dimension rows, found {len(rows)}"
        )
        indices = sorted(int(i) for i, _ in rows)
        self.assertEqual(indices, [1, 2, 3, 4, 5, 6, 7, 8, 9])
        total = sum(int(w) for _, w in rows)
        self.assertEqual(total, 44, f"dimension weights sum to {total}, not 44")

    def test_dim_one_is_pedagogy_dominant(self):
        # Pedagogical scaffolding is the OWNED, highest-weighted dim (7).
        self.assertTrue(
            re.search(
                r"^\|\s*1\s*\|\s*\*\*Pedagogical scaffolding[^|]*\*\*\s*\|\s*7\s*\|",
                self.text,
                flags=re.MULTILINE,
            ),
            "dim 1 must be Pedagogical scaffolding at weight 7",
        )

    def test_dim_one_is_the_unique_maximum(self):
        rows = re.findall(
            r"^\|\s*([1-9])\s*\|\s*\*\*[^|]+\*\*\s*\|\s*(\d+)\s*\|",
            self.text,
            flags=re.MULTILINE,
        )
        weights = {int(i): int(w) for i, w in rows}
        top = max(weights.values())
        winners = [i for i, w in weights.items() if w == top]
        self.assertEqual(
            winners, [1], f"dim 1 must be the unique dominant dim, got {winners}"
        )

    def test_advance_threshold_is_general_tier(self):
        self.assertTrue(
            re.search(
                r"threshold to advance is \*\*≥35/44\*\*",
                self.text,
                re.IGNORECASE,
            ),
            "rubric must declare the >=35/44 general advance threshold",
        )
        # Explicitly NOT the customer-facing >=39 band.
        self.assertIn("NOT the customer-facing ≥39 band", self.text)

    def test_rubric_id_declared(self):
        self.assertIn(RUBRIC_ID, self.text)

    def test_three_named_critical_flags(self):
        text = self.text
        self.assertIn("Duplicates formal spec section", text)
        self.assertIn("Contradicts cited spec", text)
        self.assertIn("Subtly-wrong intuition", text)

    def test_spec_flags_inactive_when_spec_ref_undeclared(self):
        # The two spec-consistency flags must not fire without spec_ref.
        self.assertIn("cannot fire", self.text)
        self.assertIn("undeclared", self.text)

    def test_stamping_fields_in_meta_example(self):
        self.assertIn(f'"rubric_id": "{RUBRIC_ID}"', self.text)
        self.assertIn('"rubric_total": 44', self.text)
        self.assertIn('"advance_threshold": 35', self.text)

    def test_human_verdict_scorecard_kind(self):
        self.assertIn("human-verdict", self.text)


class TestRegistryIntegration(unittest.TestCase):
    """primer is registered as a skill-identity artifact type and the
    spec_ref resolver + per-phase agents exist."""

    def _import_registry(self):
        import sys

        if str(_REPO_ROOT) not in sys.path:
            sys.path.insert(0, str(_REPO_ROOT))
        from anvil.lib import project_brief

        return project_brief

    def test_artifact_type_registered_as_skill_identity(self):
        pb = self._import_registry()
        self.assertIn("primer", pb.REGISTERED_ARTIFACT_TYPES)
        self.assertIn(pb.ArtifactType.PRIMER, pb.SKILL_IDENTITY_ARTIFACT_TYPES)
        # NOT a memo subtype — selects no memo rubric overlay.
        self.assertNotIn(pb.ArtifactType.PRIMER, pb.MEMO_ARTIFACT_TYPES)

    def test_resolve_spec_ref_is_exported(self):
        pb = self._import_registry()
        self.assertTrue(hasattr(pb, "resolve_spec_ref"))
        self.assertTrue(hasattr(pb, "ResolvedSpecRef"))

    def test_lifecycle_agents_generated(self):
        agents_dir = _REPO_ROOT / "anvil" / "agents"
        # primer borrows the full report-shaped lifecycle → all five agents.
        for name in (
            "anvil-primer-drafter.md",
            "anvil-primer-reviewer.md",
            "anvil-primer-reviser.md",
            "anvil-primer-auditor.md",
            "anvil-primer-figurer.md",
        ):
            with self.subTest(agent=name):
                self.assertTrue(
                    (agents_dir / name).is_file(),
                    f"missing agent registration {name} "
                    f"(run scripts/generate-anvil-agents.py)",
                )


class TestSpecRefResolver(unittest.TestCase):
    """resolve_spec_ref honors the declared/absent/missing activation contract."""

    def _import_registry(self):
        import sys

        if str(_REPO_ROOT) not in sys.path:
            sys.path.insert(0, str(_REPO_ROOT))
        from anvil.lib import project_brief

        return project_brief

    def _write_brief(self, project_dir: Path, spec_ref_line: str) -> None:
        (project_dir / "BRIEF.md").write_text(
            "---\n"
            "project: toy\n"
            "documents:\n"
            "  - slug: toy-primer\n"
            "    artifact_type: primer\n"
            f"{spec_ref_line}"
            "---\n\n"
            "# Toy primer project\n",
            encoding="utf-8",
        )

    def test_absent_spec_ref_is_inactive_none(self):
        import tempfile

        pb = self._import_registry()
        with tempfile.TemporaryDirectory() as d:
            project_dir = Path(d)
            self._write_brief(project_dir, spec_ref_line="")
            resolved = pb.resolve_spec_ref(
                project_dir, "toy-primer", consumer_root=project_dir
            )
            self.assertIsNone(
                resolved, "absent spec_ref must resolve to None (tier off)"
            )

    def test_declared_and_resolves(self):
        import tempfile

        pb = self._import_registry()
        with tempfile.TemporaryDirectory() as d:
            project_dir = Path(d)
            spec = project_dir / "spec.md"
            spec.write_text("# formal spec\n", encoding="utf-8")
            self._write_brief(
                project_dir, spec_ref_line="    spec_ref: spec.md\n"
            )
            resolved = pb.resolve_spec_ref(
                project_dir, "toy-primer", consumer_root=project_dir
            )
            self.assertIsNotNone(resolved)
            self.assertFalse(resolved.missing)
            self.assertEqual(len(resolved.paths), 1)
            self.assertTrue(Path(resolved.paths[0]).is_file())

    def test_declared_but_missing_activates_without_crash(self):
        import tempfile

        pb = self._import_registry()
        with tempfile.TemporaryDirectory() as d:
            project_dir = Path(d)
            # Declared path points at a file that does not exist — the
            # tier activates, resolution never raises, missing is True.
            self._write_brief(
                project_dir, spec_ref_line="    spec_ref: no-such-spec.md\n"
            )
            resolved = pb.resolve_spec_ref(
                project_dir, "toy-primer", consumer_root=project_dir
            )
            self.assertIsNotNone(
                resolved,
                "declared-but-missing spec_ref must ACTIVATE (not None)",
            )
            self.assertTrue(resolved.missing)
            self.assertEqual(resolved.paths, [])

    def test_no_matching_slug_is_inactive_none(self):
        import tempfile

        pb = self._import_registry()
        with tempfile.TemporaryDirectory() as d:
            project_dir = Path(d)
            self._write_brief(
                project_dir, spec_ref_line="    spec_ref: spec.md\n"
            )
            resolved = pb.resolve_spec_ref(
                project_dir, "not-a-slug", consumer_root=project_dir
            )
            self.assertIsNone(resolved)


if __name__ == "__main__":
    unittest.main()
