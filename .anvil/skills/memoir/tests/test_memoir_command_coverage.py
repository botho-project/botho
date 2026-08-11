"""Structural smoke tests for the ``anvil:memoir`` skill (issue #740).

These tests assert **structural properties** of the shipped skill files
(files exist, frontmatter parses, the rubric declares 9 dimensions summing
to 44 with a >=39 advance threshold under the ``anvil-memoir-v1`` id,
sourcing fidelity is the OWNED dominant dim 1 at weight 7, the reused-
verbatim #597 fabrication-class flags plus the conditional #598
misattribution flag are documented, every critic-writing command stamps
the #346 rubric fields and uses the staged-sidecar primitive, the
dual-corpus (#597) / dual-voice (#598) activation contracts are wired into
draft/review/audit/revise, the exhaustive ``kind: tool_evidence``
corpus-audit critic contract is documented, the photo-placement macro
contract is documented, and the report-shaped parallel-review+audit
AUDITED-terminal lifecycle is present). They are intentionally NOT
golden-file tests — the skill is a generative authoring skill and prose
varies across runs and models.

Runs under either ``pytest anvil/skills/memoir/tests/`` or
``python -m unittest discover anvil/skills/memoir/tests/``.

Per the #58 packaging convention this filename
(``test_memoir_command_coverage``) is unique across the
``anvil/skills/*/tests/`` tree; the package carries an ``__init__.py`` to
avoid the cross-skill pytest collection collision.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

_SKILL_ROOT = Path(__file__).resolve().parent.parent
_REPO_ROOT = _SKILL_ROOT.parents[2]

RUBRIC_ID = "anvil-memoir-v1"

# Every critic-writing command must carry the #346 stamps + the atomic
# sidecar primitive.
CRITIC_COMMANDS = ("commands/memoir-review.md", "commands/memoir-audit.md")

FABRICATION_FLAGS = (
    "fabricated_quote",
    "fabricated_fact",
    "misattribution_of_substance",
    "anachronism",
    "unattributed_paraphrase",
)


def _read(rel: str) -> str:
    return (_SKILL_ROOT / rel).read_text(encoding="utf-8")


def _flat(rel: str) -> str:
    """``_read`` with all whitespace runs collapsed to single spaces.

    Phrase assertions against hard-wrapped prose break whenever a
    paragraph reflows, which is churn rather than signal. Reading the
    flattened text lets a phrase assertion span a line break.
    """
    return re.sub(r"\s+", " ", _read(rel))


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
    """The pinned file manifest is present on disk (v1 scope, issue #740)."""

    EXPECTED = [
        "SKILL.md",
        "rubric.md",
        "README.md",
        "__init__.py",
        "commands/memoir.md",
        "commands/memoir-draft.md",
        "commands/memoir-review.md",
        "commands/memoir-audit.md",
        "commands/memoir-revise.md",
        "commands/memoir-figures.md",
        "templates/BRIEF.md.example",
        "templates/memoir.template.tex",
        "tests/__init__.py",
        "tests/test_memoir_command_coverage.py",
    ]

    def test_manifest_present(self):
        for rel in self.EXPECTED:
            with self.subTest(path=rel):
                self.assertTrue(
                    (_SKILL_ROOT / rel).exists(), f"missing skill file: {rel}"
                )

    def test_deferred_scope_absent(self):
        # Never-in-scope-for-v1 commands are absent (SKILL.md §Scope guard).
        for stem in ("memoir-cross-chapter-check", "memoir-facts-register"):
            with self.subTest(command=stem):
                self.assertFalse(
                    (_SKILL_ROOT / "commands" / f"{stem}.md").exists(),
                    f"{stem}.md is deferred/out-of-scope for v1",
                )

    def test_minimal_worked_example_vendored(self):
        # A minimal SYNTHETIC worked example (NOT the full nitas-mama
        # dogfood — that is deferred, SKILL.md §Scope guard).
        examples = _SKILL_ROOT / "examples"
        self.assertTrue(examples.is_dir(), "a minimal worked example must be vendored")
        self.assertTrue(
            (examples / "grani-memoir-mini" / "BRIEF.md").is_file(),
            "expected the vendored example BRIEF",
        )
        self.assertTrue(
            (examples / "expected-thread.N" / "README.md").is_file(),
            "expected the structural-contract README",
        )


class TestSkillFrontmatter(unittest.TestCase):
    """SKILL.md frontmatter matches the sibling skills' shape."""

    def test_frontmatter(self):
        fm = _parse_frontmatter(_read("SKILL.md"))
        self.assertEqual(fm.get("name"), "memoir")
        self.assertEqual(fm.get("domain"), "memoir")
        self.assertEqual(fm.get("type"), "skill")
        self.assertIn(fm.get("user-invocable"), (False, "false"))

    def test_report_shaped_lifecycle(self):
        text = _read("SKILL.md")
        self.assertIn("skill identity = artifact identity", text)
        self.assertIn("REVIEWED+AUDITED", text)
        self.assertIn("AUDITED", text)
        self.assertIn("no shortcut to `READY`", text)

    def test_corpus_activation_contract_documented(self):
        text = _read("SKILL.md")
        self.assertIn("resolve_corpus_dirs", text)
        self.assertIn("major", text)
        self.assertIn("silent", text.lower())
        self.assertIn("#597", text)

    def test_voice_activation_contract_documented(self):
        text = _read("SKILL.md")
        self.assertIn("resolve_subject_voice_docs", text)
        self.assertIn("#598", text)
        self.assertIn("independently activated", text)

    def test_corpus_audit_sibling_documented(self):
        text = _read("SKILL.md")
        self.assertIn("corpus-audit", text)
        self.assertIn("tool_evidence", text)
        self.assertIn("provenance.md", text)
        self.assertIn("Section 4", text)

    def test_photo_placement_contract_documented(self):
        text = _read("SKILL.md")
        for macro in ("\\famphoto", "\\fullphoto", "\\marginphoto"):
            with self.subTest(macro=macro):
                self.assertIn(macro, text)
        self.assertIn("manifest.json", text)
        self.assertIn("project-photos", text)

    def test_project_book_relationship_documented(self):
        text = _read("SKILL.md")
        self.assertIn("Relationship to `anvil:project-book`", text)
        self.assertIn("project-book", text)
        self.assertIn("portfolio", text.lower())

    def test_positioning_table_documented(self):
        text = _read("SKILL.md")
        self.assertIn(
            "Relationship to `anvil:essay` / `anvil:primer` / "
            "`anvil:spec` / `anvil:report`",
            text,
        )
        for skill in ("anvil:essay", "anvil:primer", "anvil:spec", "anvil:report"):
            with self.subTest(skill=skill):
                self.assertIn(skill, text)

    def test_latex_body_posture_documented(self):
        text = _read("SKILL.md")
        self.assertIn("LaTeX", text)
        self.assertIn(".tex", text)

    def test_scope_guard_section_present(self):
        text = _read("SKILL.md")
        self.assertIn("Scope guard", text)
        self.assertIn("v1 / deferred", text)
        self.assertIn("Deferred", text)

    def test_sidecar_stamping_and_scorecard_contracts_referenced(self):
        text = _read("SKILL.md")
        self.assertIn("staged_sidecar", text)
        self.assertIn(RUBRIC_ID, text)
        self.assertIn("human-verdict", text)


class TestCommandFrontmatter(unittest.TestCase):
    """Every command file carries a name/description frontmatter block."""

    COMMANDS = {
        "commands/memoir.md": "memoir",
        "commands/memoir-draft.md": "memoir-draft",
        "commands/memoir-review.md": "memoir-review",
        "commands/memoir-audit.md": "memoir-audit",
        "commands/memoir-revise.md": "memoir-revise",
        "commands/memoir-figures.md": "memoir-figures",
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
                self.assertIn("advance_threshold: 39", text)
                self.assertIn("staged_sidecar", text)
                self.assertIn("cleanup_one_staging", text)
                self.assertIn("human-verdict", text)


class TestDualCorpusWiring(unittest.TestCase):
    """corpus: (#597) is wired into draft / review / audit / revise."""

    def test_draft_resolves_corpus(self):
        text = _read("commands/memoir-draft.md")
        self.assertIn("resolve_corpus_dirs", text)
        self.assertIn("provenance.md", text)
        self.assertIn("corpus_dirs_resolved", text)
        self.assertIn("Fabricating a source-line mapping is prohibited", text)

    def test_review_runs_back_check(self):
        text = _read("commands/memoir-review.md")
        self.assertIn("resolve_corpus_dirs", text)
        self.assertIn("back-check", text.lower())
        self.assertIn("Spot-sample 5-10 rows", text)

    def test_audit_runs_exhaustive_sweep(self):
        text = _read("commands/memoir-audit.md")
        self.assertIn("resolve_corpus_dirs", text)
        self.assertIn("exhaustive", text.lower())
        self.assertIn("tool_evidence", text)
        for classification in (
            "VERIFIED",
            "PARAPHRASE_OK",
            "MISMATCH",
            "NOT_FOUND",
            "FABRICATED",
        ):
            with self.subTest(classification=classification):
                self.assertIn(classification, text)

    def test_revise_never_fabricates_mapping(self):
        text = _read("commands/memoir-revise.md")
        self.assertIn("provenance.md", text)
        self.assertIn("Never invent a new source-line mapping", text)

    def test_byte_identical_when_absent_documented(self):
        for rel in (
            "commands/memoir-draft.md",
            "commands/memoir-review.md",
            "commands/memoir-audit.md",
        ):
            with self.subTest(command=rel):
                text = _read(rel)
                self.assertIn("major", text)
                self.assertIn("never raises", text.lower())


class TestAnchorDriftWiring(unittest.TestCase):
    """provenance.md line-range citations get a stable Anchor identity
    and a distinct drift-detection/repoint path (issue #868)."""

    def test_draft_writes_anchor_column(self):
        text = _read("commands/memoir-draft.md")
        self.assertIn("Anchor", text)
        self.assertIn("stable identity", text.lower())

    def test_audit_runs_anchor_resolution_pre_pass(self):
        text = _read("commands/memoir-audit.md")
        self.assertIn("provenance_anchor", text)
        self.assertIn("DRIFTED", text)
        self.assertIn("anchor drift", text.lower())
        # Must read distinctly from the five-way vocabulary, not fold
        # into it.
        self.assertTrue(
            "never phrased as `mismatch`" in text.lower()
            or "never phrased as mismatch" in text.lower()
        )

    def test_revise_repoints_drifted_anchors_mechanically(self):
        text = _read("commands/memoir-revise.md")
        self.assertIn("provenance_anchor", text)
        self.assertIn("repoint", text.lower())
        self.assertIn("mechanical", text.lower())

    def test_review_back_check_is_drift_aware(self):
        text = _read("commands/memoir-review.md")
        self.assertIn("anchor drift", text.lower())

    def test_shared_snippet_documents_anchor_contract(self):
        text = (
            _REPO_ROOT / "anvil" / "lib" / "snippets" / "provenance.md"
        ).read_text(encoding="utf-8")
        self.assertIn("Anchor", text)
        self.assertIn("NO_ANCHOR", text)
        self.assertIn("DRIFTED", text)
        self.assertIn("provenance_anchor", text)
        self.assertIn("Backward compatibility", text)


class TestDualVoiceWiring(unittest.TestCase):
    """voice: / voice.subjects: (#461/#598) are wired into draft/review."""

    def test_draft_resolves_both_voice_tiers(self):
        text = _read("commands/memoir-draft.md")
        self.assertIn("resolve_voice_docs", text)
        self.assertIn("resolve_subject_voice_docs", text)
        self.assertIn("voice_exemplars", text)
        self.assertIn("subject_voice_exemplars", text)

    def test_review_scores_both_voice_tiers(self):
        text = _read("commands/memoir-review.md")
        self.assertIn("resolve_voice_docs", text)
        self.assertIn("resolve_subject_voice_docs", text)
        self.assertIn("Narrator voice fidelity", text)
        self.assertIn("Subject voice fidelity", text)

    def test_review_documents_misattribution_flag(self):
        text = _read("commands/memoir-review.md")
        self.assertIn("misattribution", text)
        self.assertIn(">=2 subjects", text)


class TestCorpusAuditSiblingContract(unittest.TestCase):
    """The exhaustive corpus-audit sibling coexists with the general audit."""

    def test_audit_writes_two_siblings(self):
        text = _read("commands/memoir-audit.md")
        self.assertIn("<thread>.{N}.audit/", text)
        self.assertIn("<thread>.{N}.corpus-audit/", text)
        self.assertIn("ALWAYS", text)

    def test_audit_documents_fabrication_flags(self):
        text = _read("commands/memoir-audit.md")
        for flag in FABRICATION_FLAGS:
            with self.subTest(flag=flag):
                self.assertIn(flag, text)

    def test_audit_never_invents_new_flag_vocabulary(self):
        text = _read("commands/memoir-audit.md")
        self.assertIn("reused verbatim", text.lower())

    def test_revise_reads_all_three_siblings(self):
        text = _read("commands/memoir-revise.md")
        self.assertIn("<thread>.{N}.review/", text)
        self.assertIn("<thread>.{N}.audit/", text)
        self.assertIn("<thread>.{N}.corpus-audit/", text)


class TestPhotoPlacementContract(unittest.TestCase):
    """The photo-placement macro contract is wired into figures + templates."""

    def test_figures_resolves_macros_against_manifest(self):
        text = _read("commands/memoir-figures.md")
        for macro in ("\\famphoto", "\\fullphoto", "\\marginphoto"):
            with self.subTest(macro=macro):
                self.assertIn(macro, text)
        self.assertIn("manifest.json", text)
        self.assertIn("render_gate.py", text)
        self.assertIn("xelatex", text.lower())

    def test_unresolved_stable_name_is_a_finding_not_a_crash(self):
        text = _read("commands/memoir-figures.md")
        self.assertIn("major", text)
        self.assertIn("never a silent placeholder", text.lower())
        self.assertIn("never a crash", text.lower())

    def test_template_defines_the_macros(self):
        text = _read("templates/memoir.template.tex")
        for macro in ("\\famphoto", "\\fullphoto", "\\marginphoto"):
            with self.subTest(macro=macro):
                self.assertIn(f"\\newcommand{{{macro}}}", text)

    def test_figures_never_mutates_manifest(self):
        text = _read("commands/memoir-figures.md")
        self.assertIn("strictly read-only", text.lower())


class TestIterationCapContract(unittest.TestCase):
    """The iteration-cap / override contract is documented (issue #869).

    The canary friction was a chapter landing at ``iteration ==
    max_iterations`` with no documented answer to "what happens next" and
    no pointer to the (already-shipped, shared) paired override. These
    assertions pin the three surfaces that answer it: the reviser's cap
    predicate + BLOCKED notice, SKILL.md's override contract, and the
    orchestrator's at-the-ceiling reporting rule.
    """

    def test_revise_states_the_cap_predicate(self):
        text = _read("commands/memoir-revise.md")
        self.assertIn("N + 1 > effective_max_iterations", text)
        # The off-by-one the issue complains about: under the predicate
        # the terminal dir is <thread>.{max_iterations}/, not .5/.
        self.assertIn("<thread>.{max_iterations}/", text)
        self.assertNotIn("worst-case terminal version `<thread>.5/`", text)

    def test_revise_documents_behavior_at_the_ceiling(self):
        flat = _flat("commands/memoir-revise.md")
        self.assertIn("iteration == max_iterations", flat)
        # Refuse / warn / proceed must be answered unambiguously.
        self.assertIn("refuses; it never warns-and-proceeds", flat)
        # A clean terminus at the ceiling is NOT blocked.
        self.assertIn("was never the *terminating condition*", flat)

    def test_revise_documents_the_blocked_notice_contract(self):
        flat = _flat("commands/memoir-revise.md")
        self.assertIn("BLOCKED notice", flat)
        self.assertIn("Override available", flat)
        self.assertIn("iteration_cap_rationale", flat)
        self.assertIn("max_iterations (int ≥ 4)", flat)
        # The already-elevated branch echoes the prior rationale in full.
        self.assertIn("in full, never truncated", flat)
        self.assertIn("already at its elevated cap", flat)
        # Budget composition (the #869-specific surfacing).
        self.assertIn("Budget composition", flat)

    def test_revise_budget_composition_example_is_self_consistent(self):
        """The worked composition line must not double-count a version.

        A phrase-level pin on ``Budget composition`` alone let a
        contradictory instance ship (a version classified as *both*
        substantive and map-only, tallying four classified passes on a
        thread that can hold at most three). This re-derives the worked
        example's arithmetic instead of trusting the prose: ``v1`` is the
        draft and carries no ``revision_class``, so the classified groups
        must be disjoint and ``1 (draft) + substantive + map-only`` must
        equal the consumed numerator.
        """
        flat = _flat("commands/memoir-revise.md")

        # The tally model is stated, not left to be inferred.
        self.assertIn("no `metadata.revision_class`", flat)
        self.assertIn("1 (draft) + substantive + map-only == X", flat)

        match = re.search(
            r"Budget composition: (\d+)/(\d+) consumed — v1 draft, "
            r"(\d+) substantive \(([^)]*)\), (\d+) map-only \(([^;)]*)[;)]",
            flat,
        )
        self.assertIsNotNone(
            match, "worked budget-composition example not found or reshaped"
        )
        consumed, cap, n_sub, sub_body, n_map, map_body = match.groups()
        consumed, cap, n_sub, n_map = (
            int(consumed),
            int(cap),
            int(n_sub),
            int(n_map),
        )

        self.assertLessEqual(consumed, cap)
        # The declared counts match the versions actually named.
        sub_versions = set(re.findall(r"v\d+", sub_body))
        map_versions = set(re.findall(r"v\d+", map_body))
        self.assertEqual(len(sub_versions), n_sub)
        self.assertEqual(len(map_versions), n_map)
        # A pass is substantive XOR map-only — never both.
        self.assertEqual(sub_versions & map_versions, set())
        # v1 is the draft; it is never classified.
        self.assertNotIn("v1", sub_versions | map_versions)
        # 1 draft + classified revisions == the consumed numerator.
        self.assertEqual(1 + n_sub + n_map, consumed)

    def test_revise_records_the_cap_audit_trail(self):
        text = _read("commands/memoir-revise.md")
        self.assertIn("metadata.max_iterations", text)
        self.assertIn("metadata.iteration_cap_rationale", text)
        self.assertIn("metadata.revision_class", text)

    def test_revise_never_raises_the_cap_itself(self):
        text = _read("commands/memoir-revise.md")
        self.assertIn("Never raises the iteration cap itself", text)

    def test_map_only_non_goal_documented_with_rationale(self):
        flat = _flat("commands/memoir-revise.md")
        self.assertIn("Map-only repairs still consume an iteration", flat)
        self.assertIn("map_only", flat)
        self.assertIn("byte-identical", flat)
        self.assertIn("deliberate design decision", flat)

    def test_draft_mirrors_the_cap_into_progress(self):
        text = _read("commands/memoir-draft.md")
        self.assertIn("max_iterations", text)
        self.assertIn("iteration_cap_rationale", text)
        self.assertIn("DEFAULT_MAX_ITERATIONS", text)

    def test_skill_documents_the_override_contract(self):
        flat = _flat("SKILL.md")
        self.assertIn("Iteration cap and override contract", flat)
        self.assertIn("iteration_cap_rationale", flat)
        self.assertIn("sticky raise", flat)
        # memo's contract is the schema-of-record; memoir reuses it.
        self.assertIn("Per-document override contract", flat)
        self.assertIn("explicit, recorded operator decision", flat)

    def test_orchestrator_distinguishes_clean_terminus_from_blocked(self):
        flat = _flat("commands/memoir.md")
        self.assertIn("N + 1 > max_iterations", flat)
        self.assertIn("is `AUDITED` is terminal and healthy", flat)
        self.assertIn("revision_class", flat)


class TestRubric(unittest.TestCase):
    """rubric.md declares 9 dims summing to 44, >=39, sourcing-fidelity
    dominant."""

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

    def test_dim_one_is_sourcing_fidelity_dominant(self):
        self.assertTrue(
            re.search(
                r"^\|\s*1\s*\|\s*\*\*Sourcing fidelity[^|]*\*\*\s*\|\s*7\s*\|",
                self.text,
                flags=re.MULTILINE,
            ),
            "dim 1 must be Sourcing fidelity at weight 7",
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

    def test_advance_threshold_is_audit_grade_band(self):
        self.assertTrue(
            re.search(
                r"threshold to advance is \*\*≥39/44\*\*",
                self.text,
                re.IGNORECASE,
            ),
            "rubric must declare the >=39/44 audit-grade advance threshold",
        )

    def test_rubric_id_declared(self):
        self.assertIn(RUBRIC_ID, self.text)

    def test_fabrication_class_flags_documented(self):
        for flag in FABRICATION_FLAGS:
            with self.subTest(flag=flag):
                self.assertIn(flag, self.text)

    def test_misattribution_flag_documented_conditional(self):
        self.assertIn("Misattribution", self.text)
        self.assertIn(">=2 subjects", self.text)

    def test_stamping_fields_in_meta_example(self):
        self.assertIn(f'"rubric_id": "{RUBRIC_ID}"', self.text)
        self.assertIn('"rubric_total": 44', self.text)
        self.assertIn('"advance_threshold": 39', self.text)

    def test_human_verdict_scorecard_kind(self):
        self.assertIn("human-verdict", self.text)

    def test_corpus_audit_sibling_in_sidecar_format(self):
        self.assertIn("<thread>.{N}.corpus-audit/", self.text)


class TestRegistryIntegration(unittest.TestCase):
    """memoir is registered as a skill-identity artifact type and the
    per-phase agents exist."""

    def _import_registry(self):
        import sys

        if str(_REPO_ROOT) not in sys.path:
            sys.path.insert(0, str(_REPO_ROOT))
        from anvil.lib import project_brief

        return project_brief

    def test_artifact_type_registered_as_skill_identity(self):
        pb = self._import_registry()
        self.assertIn("memoir", pb.REGISTERED_ARTIFACT_TYPES)
        self.assertIn(pb.ArtifactType.MEMOIR, pb.SKILL_IDENTITY_ARTIFACT_TYPES)
        # NOT a memo subtype — selects no memo rubric overlay.
        self.assertNotIn(pb.ArtifactType.MEMOIR, pb.MEMO_ARTIFACT_TYPES)

    def test_corpus_and_voice_resolvers_already_general(self):
        # #597/#598 need zero anvil/lib/ changes for memoir — both
        # resolvers already exist and are exported.
        pb = self._import_registry()
        self.assertTrue(hasattr(pb, "resolve_corpus_dirs"))
        self.assertTrue(hasattr(pb, "resolve_voice_docs"))
        self.assertTrue(hasattr(pb, "resolve_subject_voice_docs"))

    def test_lifecycle_agents_generated(self):
        agents_dir = _REPO_ROOT / "anvil" / "agents"
        for name in (
            "anvil-memoir-drafter.md",
            "anvil-memoir-reviewer.md",
            "anvil-memoir-reviser.md",
            "anvil-memoir-auditor.md",
            "anvil-memoir-figurer.md",
        ):
            with self.subTest(agent=name):
                self.assertTrue(
                    (agents_dir / name).is_file(),
                    f"missing agent registration {name} "
                    f"(run scripts/generate-anvil-agents.py)",
                )


class TestCorpusResolver(unittest.TestCase):
    """resolve_corpus_dirs honors the declared/absent/missing activation
    contract when a memoir BRIEF declares corpus: (project-level)."""

    def _import_registry(self):
        import sys

        if str(_REPO_ROOT) not in sys.path:
            sys.path.insert(0, str(_REPO_ROOT))
        from anvil.lib import project_brief

        return project_brief

    def _write_brief(self, project_dir: Path, corpus_block: str) -> None:
        (project_dir / "BRIEF.md").write_text(
            "---\n"
            "project: toy\n"
            f"{corpus_block}"
            "documents:\n"
            "  - slug: 00-introduction\n"
            "    artifact_type: memoir\n"
            "---\n\n"
            "# Toy memoir project\n",
            encoding="utf-8",
        )

    def test_absent_corpus_is_inactive_empty_list(self):
        import tempfile

        pb = self._import_registry()
        with tempfile.TemporaryDirectory() as d:
            project_dir = Path(d)
            self._write_brief(project_dir, corpus_block="")
            resolved = pb.resolve_corpus_dirs(project_dir, consumer_root=project_dir)
            self.assertEqual(resolved, [])

    def test_declared_and_resolves(self):
        import tempfile

        pb = self._import_registry()
        with tempfile.TemporaryDirectory() as d:
            project_dir = Path(d)
            (project_dir / "transcripts").mkdir()
            (project_dir / "letters").mkdir()
            self._write_brief(
                project_dir,
                corpus_block="corpus:\n  - transcripts/\n  - letters/\n",
            )
            resolved = pb.resolve_corpus_dirs(project_dir, consumer_root=project_dir)
            self.assertEqual(len(resolved), 2)
            self.assertFalse(any(r.missing for r in resolved))

    def test_declared_but_missing_activates_without_crash(self):
        import tempfile

        pb = self._import_registry()
        with tempfile.TemporaryDirectory() as d:
            project_dir = Path(d)
            self._write_brief(
                project_dir, corpus_block="corpus:\n  - no-such-dir/\n"
            )
            resolved = pb.resolve_corpus_dirs(project_dir, consumer_root=project_dir)
            self.assertEqual(len(resolved), 1)
            self.assertTrue(resolved[0].missing)

    def test_declares_memoir_artifact_type(self):
        pb = self._import_registry()
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            project_dir = Path(d)
            self._write_brief(project_dir, corpus_block="")
            brief = pb.load_project_brief_strict(project_dir)
            doc = next(d for d in brief.documents if d.slug == "00-introduction")
            self.assertEqual(doc.artifact_type, pb.ArtifactType.MEMOIR)


if __name__ == "__main__":
    unittest.main()
