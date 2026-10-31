"""Tests for proposal ``load_cost_basis`` (issue #840).

The helper reads the ``cost_basis`` frontmatter key on a proposal
thread-level ``<thread>/BRIEF.md`` and resolves it to a typed signal
the drafter, reviewer, and auditor dispatch on for the priced-table
contract and dim 6 (Cost credibility) scoring. Per the issue body, the
function:

1. Returns the value verbatim for each of the closed-set members
   (``quoted`` / ``estimated`` / ``none``).
2. Returns ``None`` for every absence / malformed path (missing BRIEF,
   no frontmatter, malformed YAML, missing key, value not in the
   closed set including typos like ``Quoted`` or ``vendor``).
3. Never raises — lenient by design, mirroring
   ``load_recommendation_target``'s contract.

The unique filename (``test_brief_cost_basis.py``) avoids collision
with other skills' tests per the #58 packaging convention.

Runs under either ``python -m unittest discover anvil/skills/proposal/tests/``
or ``pytest anvil/skills/proposal/tests/``.
"""

from __future__ import annotations

import importlib.util
import textwrap
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory


# Load the proposal skill's project_brief.py by explicit file path under a
# skill-qualified module name, rather than the `sys.path.insert` + bare
# `import project_brief` shim used elsewhere in this tree. Both memo/lib/ and
# proposal/lib/ ship a same-named `project_brief.py`; when a whole-repo pytest
# session collects memo's tests before proposal's, a bare `import
# project_brief` here would silently resolve to whatever module already sits
# in `sys.modules` under that name (memo's shim, re-exporting the shared
# `anvil.lib.project_brief`) instead of this skill's own module — masking a
# collision rather than raising, since the two modules happen to share several
# function names. `load_cost_basis` does not exist on the shared module, so
# that particular masking would surface here as a hard `ImportError` instead
# of a silently-wrong test target. Loading by explicit path under a unique
# name (`_proposal_project_brief`) sidesteps the collision entirely and pins
# this test to the exact file it is meant to exercise.
_HERE = Path(__file__).resolve().parent
_MODULE_PATH = _HERE.parent / "lib" / "project_brief.py"
_spec = importlib.util.spec_from_file_location(
    "_proposal_project_brief", _MODULE_PATH
)
assert _spec is not None and _spec.loader is not None
_project_brief = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_project_brief)

BRIEF_FILENAME = _project_brief.BRIEF_FILENAME
load_cost_basis = _project_brief.load_cost_basis


class _TmpThreadBase(unittest.TestCase):
    """Per-test temp dir mimicking a proposal thread root."""

    def setUp(self) -> None:
        self._td = TemporaryDirectory()
        self.thread_dir = Path(self._td.name) / "partner-integration"
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


class TestLoadCostBasisAbsencePaths(_TmpThreadBase):
    """The lenient contract — None on every absence path, never raises."""

    def test_missing_brief_returns_none(self) -> None:
        # No BRIEF.md written.
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_missing_thread_dir_returns_none(self) -> None:
        # The thread directory itself doesn't exist.
        missing = self.thread_dir / "does-not-exist"
        self.assertIsNone(load_cost_basis(missing))

    def test_brief_with_no_frontmatter_returns_none(self) -> None:
        # Body-only BRIEF — no `---` delimiters.
        self._write_brief("# Brief title\n\nFreeform prose with no frontmatter.\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_brief_with_unclosed_frontmatter_returns_none(self) -> None:
        # Opening `---` but no closing delimiter — _extract_frontmatter returns None.
        self._write_brief("---\ncost_basis: none\n# Brief title\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_brief_with_malformed_yaml_returns_none(self) -> None:
        # YAML that fails to parse.
        body = "---\ncost_basis: : [bad\n---\n\n# body\n"
        self._write_brief(body)
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_brief_with_frontmatter_but_missing_key_returns_none(self) -> None:
        # Valid frontmatter, no cost_basis.
        body = textwrap.dedent(
            """\
            ---
            title: "Partner Integration"
            customer_kind: external
            ---

            # Brief
            """
        )
        self._write_brief(body)
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_brief_with_frontmatter_as_list_returns_none(self) -> None:
        # Frontmatter that parses to a list, not a dict.
        body = "---\n- item1\n- item2\n---\n\n# Brief\n"
        self._write_brief(body)
        self.assertIsNone(load_cost_basis(self.thread_dir))


# ---------------------------------------------------------------------------
# Closed-set validation — only the three registered values pass; everything
# else (typos, capitalization variants, types) resolves to None.
# ---------------------------------------------------------------------------


class TestLoadCostBasisClosedSet(_TmpThreadBase):
    """The closed set is the contract — typos / case variants / bad types return None."""

    def test_typo_quoted_capitalized_returns_none(self) -> None:
        self._write_brief("---\ncost_basis: Quoted\n---\n\n# Brief\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_typo_vendor_returns_none(self) -> None:
        self._write_brief("---\ncost_basis: vendor\n---\n\n# Brief\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_typo_tbd_returns_none(self) -> None:
        self._write_brief("---\ncost_basis: tbd\n---\n\n# Brief\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_value_as_integer_returns_none(self) -> None:
        self._write_brief("---\ncost_basis: 42\n---\n\n# Brief\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_value_as_list_returns_none(self) -> None:
        self._write_brief("---\ncost_basis: [quoted, estimated]\n---\n\n# Brief\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_value_as_null_returns_none(self) -> None:
        # Explicit YAML null.
        self._write_brief("---\ncost_basis: null\n---\n\n# Brief\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))

    def test_value_as_bool_returns_none(self) -> None:
        # A boolean True/False in the slot — yaml.safe_load parses as bool.
        self._write_brief("---\ncost_basis: true\n---\n\n# Brief\n")
        self.assertIsNone(load_cost_basis(self.thread_dir))


# ---------------------------------------------------------------------------
# Happy paths — each closed-set value parses verbatim.
# ---------------------------------------------------------------------------


class TestLoadCostBasisHappyPaths(_TmpThreadBase):
    """Each registered value parses verbatim from a well-formed BRIEF."""

    def test_quoted_returns_quoted(self) -> None:
        self._write_brief("---\ncost_basis: quoted\n---\n\n# Brief\n")
        self.assertEqual(load_cost_basis(self.thread_dir), "quoted")

    def test_estimated_returns_estimated(self) -> None:
        self._write_brief("---\ncost_basis: estimated\n---\n\n# Brief\n")
        self.assertEqual(load_cost_basis(self.thread_dir), "estimated")

    def test_none_string_returns_none_literal(self) -> None:
        # The string "none" (a valid closed-set member), NOT to be confused
        # with the Python `None` sentinel returned on absence.
        self._write_brief("---\ncost_basis: none\n---\n\n# Brief\n")
        self.assertEqual(load_cost_basis(self.thread_dir), "none")

    def test_quoted_string_value_returns_verbatim(self) -> None:
        # YAML allows quoted values; they normalize to bare strings.
        self._write_brief('---\ncost_basis: "estimated"\n---\n\n# Brief\n')
        self.assertEqual(load_cost_basis(self.thread_dir), "estimated")

    def test_with_other_frontmatter_keys_returns_value(self) -> None:
        # The helper extracts only the one key; surrounding keys are ignored.
        body = textwrap.dedent(
            """\
            ---
            title: "Partner Integration Proposal"
            subtitle: "Data-backed challenge"
            studio: "Test Studio"
            customer_kind: external
            orientation: portrait
            recommendation_target: undecided
            cost_basis: none
            stage: "DESIGN PROPOSAL --- CONCEPT STAGE"
            ---

            # Brief
            """
        )
        self._write_brief(body)
        self.assertEqual(load_cost_basis(self.thread_dir), "none")


# ---------------------------------------------------------------------------
# Never raises — defensive contract guards
# ---------------------------------------------------------------------------


class TestLoadCostBasisNeverRaises(_TmpThreadBase):
    """Even on adversarial inputs the helper is contractually lenient — never raises."""

    def test_string_input_is_coerced_to_path(self) -> None:
        # Callers may pass a string by accident; the helper should be lenient.
        result = load_cost_basis(str(self.thread_dir))  # type: ignore[arg-type]
        # No BRIEF written → None, NOT an exception.
        self.assertIsNone(result)

    def test_none_input_returns_none(self) -> None:
        # An adversarial caller passes None; the helper should not raise.
        result = load_cost_basis(None)  # type: ignore[arg-type]
        self.assertIsNone(result)

    def test_empty_brief_returns_none(self) -> None:
        # Completely empty file — no frontmatter, no body.
        self._write_brief("")
        self.assertIsNone(load_cost_basis(self.thread_dir))


# ---------------------------------------------------------------------------
# Template integration — the shipped BRIEF.md.example carries
# cost_basis: quoted per issue #840 (the byte-identical-compatible default).
# ---------------------------------------------------------------------------


class TestTemplateIntegration(unittest.TestCase):
    """The shipped proposal BRIEF template parses through the helper end-to-end."""

    def test_shipped_template_resolves_to_quoted(self) -> None:
        """The shipped BRIEF.md.example demonstrates the quoted default."""
        with TemporaryDirectory() as td:
            thread_dir = Path(td) / "demo-thread"
            thread_dir.mkdir()
            # Copy the shipped template into a thread-shaped layout.
            template = _HERE.parent / "templates" / "BRIEF.md.example"
            assert template.is_file(), (
                "missing template BRIEF.md.example — the integration "
                "test depends on the shipped example"
            )
            (thread_dir / BRIEF_FILENAME).write_text(
                template.read_text(encoding="utf-8"), encoding="utf-8"
            )
            self.assertEqual(
                load_cost_basis(thread_dir),
                "quoted",
                "the shipped BRIEF.md.example MUST carry "
                "`cost_basis: quoted` as the documented, byte-identical-"
                "compatible default (issue #840)",
            )


if __name__ == "__main__":
    unittest.main()
