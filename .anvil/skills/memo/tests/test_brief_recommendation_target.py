"""Tests for ``load_recommendation_target`` (issue #348).

The helper promotes the informal ``recommendation_target`` frontmatter
key on a thread-level ``<thread>/BRIEF.md`` into a typed signal the
reviewer can dispatch on. Per the issue body, the function:

1. Returns the value verbatim for each of the closed-set members
   (``invest`` / ``pass`` / ``conditional`` / ``undecided``).
2. Returns ``None`` for every absence / malformed path (missing BRIEF,
   no frontmatter, malformed YAML, missing key, value not in the
   closed set including typos like ``Undecided`` or ``tbd``).
3. Never raises — lenient by design, mirroring
   ``load_rubric_overrides_for_slug``'s contract.

Also covers ``load_recommendation_target_resolved`` (issue #837), the
dual-surface resolver that falls back to the project-root ``BRIEF.md``
``documents:`` entry when no thread-level ``BRIEF.md`` exists — the
#348 mechanism was dead code for any project migrated to the post-
#295/#296 project-first layout, since that layout has no thread-level
BRIEF at all.

The unique filename (``test_brief_recommendation_target.py``) avoids
collision with other skills' tests per the #58 packaging convention.

Runs under either ``python -m unittest discover anvil/skills/memo/tests/``
or ``pytest anvil/skills/memo/tests/``.
"""

from __future__ import annotations

import sys
import textwrap
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory


# Mirror the sys.path shim used in test_brief_rubric_overrides.py so the
# skill-local lib imports cleanly without a package install step.
_HERE = Path(__file__).resolve().parent
_LIB = _HERE.parent / "lib"
sys.path.insert(0, str(_LIB))

from project_brief import (  # noqa: E402
    load_recommendation_target,
    load_recommendation_target_resolved,
)
from project_discovery import BRIEF_FILENAME  # noqa: E402


class _TmpThreadBase(unittest.TestCase):
    """Per-test temp dir mimicking a thread root."""

    def setUp(self) -> None:
        self._td = TemporaryDirectory()
        self.thread_dir = Path(self._td.name) / "investment-memo"
        self.thread_dir.mkdir(parents=True, exist_ok=True)
        self.addCleanup(self._td.cleanup)

    def _write_brief(self, body: str) -> Path:
        """Write the given verbatim body to ``<thread>/BRIEF.md`` and return the path."""
        brief = self.thread_dir / BRIEF_FILENAME
        brief.write_text(body, encoding="utf-8")
        return brief


# ---------------------------------------------------------------------------
# Absence paths — every absence/malformed shape returns None, never raises
# ---------------------------------------------------------------------------


class TestLoadRecommendationTargetAbsencePaths(_TmpThreadBase):
    """The lenient contract — None on every absence path, never raises."""

    def test_missing_brief_returns_none(self) -> None:
        # No BRIEF.md written.
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_missing_thread_dir_returns_none(self) -> None:
        # The thread directory itself doesn't exist.
        missing = self.thread_dir / "does-not-exist"
        self.assertIsNone(load_recommendation_target(missing))

    def test_brief_with_no_frontmatter_returns_none(self) -> None:
        # Body-only BRIEF — no `---` delimiters.
        self._write_brief("# Brief title\n\nFreeform prose with no frontmatter.\n")
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_brief_with_unclosed_frontmatter_returns_none(self) -> None:
        # Opening `---` but no closing delimiter — _extract_frontmatter returns None.
        self._write_brief("---\nrecommendation_target: undecided\n# Brief title\n")
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_brief_with_malformed_yaml_returns_none(self) -> None:
        # YAML that fails to parse.
        body = "---\nrecommendation_target: : [bad\n---\n\n# body\n"
        self._write_brief(body)
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_brief_with_frontmatter_but_missing_key_returns_none(self) -> None:
        # Valid frontmatter, no recommendation_target.
        body = textwrap.dedent(
            """\
            ---
            company: "Hearth & Crumb Provisions"
            sector: "consumer / specialty food"
            ---

            # Brief
            """
        )
        self._write_brief(body)
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_brief_with_frontmatter_as_list_returns_none(self) -> None:
        # Frontmatter that parses to a list, not a dict.
        body = "---\n- item1\n- item2\n---\n\n# Brief\n"
        self._write_brief(body)
        self.assertIsNone(load_recommendation_target(self.thread_dir))


# ---------------------------------------------------------------------------
# Closed-set validation — only the four registered values pass; everything
# else (typos, capitalization variants, types) resolves to None.
# ---------------------------------------------------------------------------


class TestLoadRecommendationTargetClosedSet(_TmpThreadBase):
    """The closed set is the contract — typos / case variants / bad types return None."""

    def test_typo_undecided_capitalized_returns_none(self) -> None:
        self._write_brief("---\nrecommendation_target: Undecided\n---\n\n# Brief\n")
        # Capitalized "Undecided" is not in the closed set → None.
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_typo_tbd_returns_none(self) -> None:
        self._write_brief("---\nrecommendation_target: tbd\n---\n\n# Brief\n")
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_typo_question_mark_returns_none(self) -> None:
        self._write_brief('---\nrecommendation_target: "?"\n---\n\n# Brief\n')
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_typo_maybe_returns_none(self) -> None:
        self._write_brief("---\nrecommendation_target: maybe\n---\n\n# Brief\n")
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_value_as_integer_returns_none(self) -> None:
        # An integer in the slot — coerced/parsed by YAML as int, rejected.
        self._write_brief("---\nrecommendation_target: 42\n---\n\n# Brief\n")
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_value_as_list_returns_none(self) -> None:
        self._write_brief(
            "---\nrecommendation_target: [invest, pass]\n---\n\n# Brief\n"
        )
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_value_as_null_returns_none(self) -> None:
        # Explicit YAML null.
        self._write_brief("---\nrecommendation_target: null\n---\n\n# Brief\n")
        self.assertIsNone(load_recommendation_target(self.thread_dir))

    def test_value_as_bool_returns_none(self) -> None:
        # A boolean True/False in the slot — yaml.safe_load parses as bool.
        self._write_brief("---\nrecommendation_target: true\n---\n\n# Brief\n")
        self.assertIsNone(load_recommendation_target(self.thread_dir))


# ---------------------------------------------------------------------------
# Happy paths — each closed-set value parses verbatim.
# ---------------------------------------------------------------------------


class TestLoadRecommendationTargetHappyPaths(_TmpThreadBase):
    """Each registered value parses verbatim from a well-formed BRIEF."""

    def test_undecided_returns_undecided(self) -> None:
        body = textwrap.dedent(
            """\
            ---
            company: "Hearth & Crumb Provisions"
            sector: "consumer / specialty food"
            stage: "pre-seed"
            check_size: "$250–500K SAFE"
            recommendation_target: undecided
            ---

            # Brief: Hearth & Crumb Provisions

            Body prose.
            """
        )
        self._write_brief(body)
        self.assertEqual(load_recommendation_target(self.thread_dir), "undecided")

    def test_invest_returns_invest(self) -> None:
        self._write_brief("---\nrecommendation_target: invest\n---\n\n# Brief\n")
        self.assertEqual(load_recommendation_target(self.thread_dir), "invest")

    def test_pass_returns_pass(self) -> None:
        self._write_brief("---\nrecommendation_target: pass\n---\n\n# Brief\n")
        self.assertEqual(load_recommendation_target(self.thread_dir), "pass")

    def test_conditional_returns_conditional(self) -> None:
        self._write_brief("---\nrecommendation_target: conditional\n---\n\n# Brief\n")
        self.assertEqual(load_recommendation_target(self.thread_dir), "conditional")

    def test_quoted_string_value_returns_verbatim(self) -> None:
        # YAML allows quoted values; they normalize to bare strings.
        self._write_brief(
            '---\nrecommendation_target: "undecided"\n---\n\n# Brief\n'
        )
        self.assertEqual(load_recommendation_target(self.thread_dir), "undecided")

    def test_with_other_frontmatter_keys_returns_value(self) -> None:
        # The helper extracts only the one key; surrounding keys are ignored.
        body = textwrap.dedent(
            """\
            ---
            company: "Test Co"
            sector: "tech"
            stage: "seed"
            check_size: "$1M"
            recommendation_target: invest
            audience: ["primary", "secondary"]
            ---

            # Brief
            """
        )
        self._write_brief(body)
        self.assertEqual(load_recommendation_target(self.thread_dir), "invest")


# ---------------------------------------------------------------------------
# Never raises — defensive contract guards
# ---------------------------------------------------------------------------


class TestLoadRecommendationTargetNeverRaises(_TmpThreadBase):
    """Even on adversarial inputs the helper is contractually lenient — never raises."""

    def test_string_input_is_coerced_to_path(self) -> None:
        # Callers may pass a string by accident; the helper should be lenient.
        result = load_recommendation_target(str(self.thread_dir))  # type: ignore[arg-type]
        # No BRIEF written → None, NOT an exception.
        self.assertIsNone(result)

    def test_none_input_returns_none(self) -> None:
        # An adversarial caller passes None; the helper should not raise.
        result = load_recommendation_target(None)  # type: ignore[arg-type]
        self.assertIsNone(result)

    def test_empty_brief_returns_none(self) -> None:
        # Completely empty file — no frontmatter, no body.
        self._write_brief("")
        self.assertIsNone(load_recommendation_target(self.thread_dir))


# ---------------------------------------------------------------------------
# Template integration — the shipped fresh-thread example carries
# recommendation_target: undecided per issue #136.
# ---------------------------------------------------------------------------


class TestTemplateIntegration(unittest.TestCase):
    """The shipped fresh-thread template parses through the helper end-to-end."""

    def test_fresh_template_resolves_to_undecided(self) -> None:
        """The shipped BRIEF.fresh.md.example demonstrates the undecided default."""
        with TemporaryDirectory() as td:
            thread_dir = Path(td) / "demo-thread"
            thread_dir.mkdir()
            # Copy the shipped fresh template into a thread-shaped layout.
            template = (
                _HERE.parent / "templates" / "BRIEF.fresh.md.example"
            )
            assert template.is_file(), (
                "missing template BRIEF.fresh.md.example — the integration "
                "test depends on the shipped example"
            )
            (thread_dir / BRIEF_FILENAME).write_text(
                template.read_text(encoding="utf-8"), encoding="utf-8"
            )
            self.assertEqual(
                load_recommendation_target(thread_dir),
                "undecided",
                "the fresh-thread template MUST carry "
                "`recommendation_target: undecided` as the documented default "
                "(issue #136 + #348)",
            )


# ---------------------------------------------------------------------------
# load_recommendation_target_resolved — dual-surface resolution (issue #837)
# ---------------------------------------------------------------------------


class _TmpProjectFirstBase(unittest.TestCase):
    """Per-test temp dir mimicking the post-#295/#296 project-first shape.

    ``self.project_dir`` is the project root (holds the project-level
    ``BRIEF.md``); ``self.thread_dir`` is ``<project_dir>/<slug>`` (the
    directory that WOULD hold a legacy thread-level ``BRIEF.md``, and
    always holds the ``<slug>.{N}/`` version dirs).
    """

    SLUG = "memo"

    def setUp(self) -> None:
        self._td = TemporaryDirectory()
        self.project_dir = Path(self._td.name) / "project"
        self.thread_dir = self.project_dir / self.SLUG
        self.thread_dir.mkdir(parents=True, exist_ok=True)
        self.addCleanup(self._td.cleanup)

    def _write_project_brief(self, *, recommendation_target: str | None) -> Path:
        """Write a project-root ``BRIEF.md`` with one ``documents:`` entry.

        ``recommendation_target`` is interpolated verbatim into the
        matching document entry when not ``None``; omitted entirely
        when ``None`` (the "no per-doc declaration" case).
        """
        rt_line = (
            f"    recommendation_target: {recommendation_target}\n"
            if recommendation_target is not None
            else ""
        )
        body = textwrap.dedent(
            f"""\
            ---
            project: demo-project
            audience:
              - demo audience
            hard_rules: []
            documents:
              - slug: {self.SLUG}
                artifact_type: investment-memo
            {rt_line}---

            # Project BRIEF
            """
        )
        brief = self.project_dir / BRIEF_FILENAME
        brief.write_text(body, encoding="utf-8")
        return brief

    def _write_thread_brief(self, body: str) -> Path:
        brief = self.thread_dir / BRIEF_FILENAME
        brief.write_text(body, encoding="utf-8")
        return brief


class TestResolvedProjectFirstShape(_TmpProjectFirstBase):
    """The #837 acceptance criterion: project-first-shape fixture.

    A project-root ``BRIEF.md`` with a ``documents:`` entry carrying
    ``recommendation_target: undecided`` and NO thread-level
    ``BRIEF.md`` — the exact shape the canary reported as dead code.
    """

    def test_project_level_undecided_fires_with_project_source(self) -> None:
        # No thread-level BRIEF.md written — matches the canary's
        # "all-dogs-go-to-heaven memo thread" reproduction shape.
        self._write_project_brief(recommendation_target="undecided")
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertEqual(value, "undecided")
        self.assertEqual(source, "project")

    def test_project_level_invest_resolves_with_project_source(self) -> None:
        self._write_project_brief(recommendation_target="invest")
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertEqual(value, "invest")
        self.assertEqual(source, "project")

    def test_project_level_typo_resolves_to_default(self) -> None:
        # Invalid value on the per-doc field normalizes to None at
        # parse time (the field's deliberate lenient-validation
        # exception) — the resolver sees no usable value and falls
        # through to "default", not a parse error.
        self._write_project_brief(recommendation_target="Undecided")
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertIsNone(value)
        self.assertEqual(source, "default")


class TestResolvedThreadPrecedence(_TmpProjectFirstBase):
    """Thread-level value wins over a project-level value when BOTH present."""

    def test_thread_wins_over_project(self) -> None:
        self._write_project_brief(recommendation_target="invest")
        self._write_thread_brief(
            "---\nrecommendation_target: conditional\n---\n\n# Brief\n"
        )
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertEqual(value, "conditional")
        self.assertEqual(source, "thread")

    def test_legacy_thread_only_shape_unaffected(self) -> None:
        # No project-level BRIEF.md at all — the pre-#295/#296 legacy
        # shape. Byte-identical to load_recommendation_target's result.
        self._write_thread_brief(
            "---\nrecommendation_target: undecided\n---\n\n# Brief\n"
        )
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertEqual(value, "undecided")
        self.assertEqual(source, "thread")


class TestResolvedDefaultPaths(_TmpProjectFirstBase):
    """Neither surface supplies a value — resolves to (None, "default")."""

    def test_no_briefs_at_all(self) -> None:
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertIsNone(value)
        self.assertEqual(source, "default")

    def test_project_brief_without_matching_slug(self) -> None:
        # Project BRIEF exists but declares a different slug.
        body = textwrap.dedent(
            """\
            ---
            project: demo-project
            audience:
              - demo audience
            hard_rules: []
            documents:
              - slug: some-other-thread
                artifact_type: investment-memo
                recommendation_target: undecided
            ---

            # Project BRIEF
            """
        )
        (self.project_dir / BRIEF_FILENAME).write_text(body, encoding="utf-8")
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertIsNone(value)
        self.assertEqual(source, "default")

    def test_project_brief_with_slug_but_no_recommendation_target(self) -> None:
        self._write_project_brief(recommendation_target=None)
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertIsNone(value)
        self.assertEqual(source, "default")

    def test_structurally_invalid_project_brief_degrades_to_default(self) -> None:
        # A project-level BRIEF that fails schema validation (missing
        # required `artifact_type`) must degrade to default rather
        # than raise — mirrors load_rubric_overrides_for_slug's
        # "malformed BRIEF never breaks the reviewer" posture.
        body = textwrap.dedent(
            """\
            ---
            project: demo-project
            audience:
              - demo audience
            hard_rules: []
            documents:
              - slug: memo
            ---

            # Project BRIEF
            """
        )
        (self.project_dir / BRIEF_FILENAME).write_text(body, encoding="utf-8")
        value, source = load_recommendation_target_resolved(self.thread_dir)
        self.assertIsNone(value)
        self.assertEqual(source, "default")


class TestResolvedNeverRaises(_TmpProjectFirstBase):
    """Adversarial inputs never raise — mirrors load_recommendation_target."""

    def test_string_thread_dir_is_coerced_to_path(self) -> None:
        value, source = load_recommendation_target_resolved(
            str(self.thread_dir)  # type: ignore[arg-type]
        )
        self.assertIsNone(value)
        self.assertEqual(source, "default")

    def test_explicit_project_dir_and_slug_override_defaults(self) -> None:
        # Caller passes project_dir / slug explicitly instead of relying
        # on the thread_dir.parent / thread_dir.name defaults — covers
        # the documented override path for divergent on-disk layouts.
        self._write_project_brief(recommendation_target="pass")
        value, source = load_recommendation_target_resolved(
            self.thread_dir,
            project_dir=self.project_dir,
            slug=self.SLUG,
        )
        self.assertEqual(value, "pass")
        self.assertEqual(source, "project")


if __name__ == "__main__":
    unittest.main()
