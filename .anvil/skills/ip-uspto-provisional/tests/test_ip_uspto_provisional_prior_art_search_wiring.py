"""Tests for the opt-in ``ip-search`` pre-search wiring (issue #958 / #978).

Issue #958 wires the ``anvil:ip-search`` utility skill (#957) into the
``ip-uspto-provisional`` prior-art critic as an **opt-in, off-by-default**
step that runs immediately before the critic's own operator-supplied-art
check — the provisional-side mirror of the same wiring in
``anvil:ip-uspto`` (see ``test_ip_uspto_prior_art_search_wiring.py``).
Issue #978 upgraded the enforcement of that step's safety properties from
followed prose to actual code — ``anvil/skills/ip-search/lib/prior_art_step.py``
— so ``TestCommandFile`` / ``TestSkillMd`` / ``TestPortfolioOrchestrator`` /
``TestReadme`` remain structural/documentation tests (the command files are
still prose procedures for an LLM-driven agent, not executable code —
mirroring the convention already used for the opt-in ``claimseed`` /
``vision`` critics in this skill), while ``TestCodeDelegation`` is a
behavioral bridge check that the doc's delegation target is real,
importable code — the full 26-case behavioral suite for the module lives
in ``anvil/skills/ip-search/tests/test_ip_search_prior_art_step.py``.

The module filename is deliberately distinct
(``test_ip_uspto_provisional_prior_art_search_wiring``) per the issue #58
cross-skill collection convention; this tests dir carries no
``__init__.py`` (``ip-uspto-provisional`` is not a valid Python package
name).
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_SKILL_ROOT = _HERE.parent
_REPO_ROOT = _HERE.parents[3]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from anvil.lib.testing import read_text

_read = lambda rel: read_text(_SKILL_ROOT / rel)


class TestCommandFile(unittest.TestCase):
    """anvil/skills/ip-uspto-provisional/commands/ip-uspto-provisional-prior-art.md."""

    REL = "commands/ip-uspto-provisional-prior-art.md"

    def setUp(self):
        self.text = _read(self.REL)

    def test_opt_in_section_present(self):
        self.assertIn("## Opt-in pre-search (issue #958)", self.text)
        self.assertIn("off by default", self.text.lower())

    def test_both_triggers_documented(self):
        self.assertIn("--search", self.text)
        self.assertIn(".anvil.json", self.text)
        self.assertIn("prior_art_search", self.text)
        self.assertIn('"auto": true', self.text)

    def test_config_passthrough_keys(self):
        for key in ("corpus", "max", "min_score"):
            self.assertIn(key, self.text)

    def test_step_1a_delegates_to_the_real_module(self):
        """Issue #978: step 1a names the code module, not just prose."""
        self.assertIn("prior_art_step.py", self.text)
        self.assertIn("run_step", self.text)
        self.assertIn("#978", self.text)

    def test_step_1a_precedes_supply_check(self):
        self.assertIn("1a.", self.text)
        idx_1a = self.text.index("1a. **Opt-in pre-search**")
        idx_step2 = self.text.index("2. **Check prior-art supply**")
        self.assertLess(idx_1a, idx_step2, "step 1a must be documented before step 2")
        self.assertIn("anvil:ip-search <thread>", self.text)

    def test_never_forces_overwrite(self):
        self.assertIn("never", self.text.lower())
        self.assertIn("--force", self.text)

    def test_degraded_path_non_fatal(self):
        self.assertIn("degraded", self.text)
        self.assertIn("non-fatal", self.text.lower())

    def test_no_art_supplied_path_preserved(self):
        self.assertIn("Dimension 5", self.text)
        self.assertIn("null", self.text.lower())

    def test_off_by_default_skip_language(self):
        self.assertIn("skipped entirely", self.text.lower())

    def test_ip_search_957_cross_referenced(self):
        self.assertIn("#957", self.text)
        self.assertIn("#958", self.text)


class TestSkillMd(unittest.TestCase):
    """SKILL.md documents the dispatch-row flag + the opt-in knob."""

    def setUp(self):
        self.text = _read("SKILL.md")

    def test_dispatch_row_carries_flag(self):
        self.assertIn("`ip-uspto-provisional-prior-art <thread> [--search]`", self.text)

    def test_opt_in_paragraph_present(self):
        self.assertIn("Optional prior-art pre-search wiring", self.text)
        self.assertIn("prior_art_search", self.text)
        self.assertIn("off by default", self.text.lower())
        self.assertIn("--force", self.text)

    def test_caveat_updated(self):
        section = self.text.split(
            "The prior-art critic does NOT do its own patent search", 1
        )[1]
        self.assertIn("by default", section[:400])
        self.assertIn("#958", section[:400])


class TestPortfolioOrchestrator(unittest.TestCase):
    """ip-uspto-provisional.md's Configuration discovery lists the new key."""

    def setUp(self):
        self.text = _read("commands/ip-uspto-provisional.md")

    def test_config_example_has_key(self):
        section = self.text.split("## Configuration discovery", 1)[1]
        self.assertIn("prior_art_search", section)
        self.assertIn('"auto": true', section)

    def test_config_bullet_documents_default_off(self):
        section = self.text.split("## Configuration discovery", 1)[1]
        self.assertIn("prior_art_search.auto", section)
        self.assertIn("default `false`", section)


class TestReadme(unittest.TestCase):
    def setUp(self):
        self.text = _read("README.md")

    def test_caveat_mentions_ip_search_and_knob(self):
        self.assertIn("anvil:ip-search", self.text)
        self.assertIn("prior_art_search.auto", self.text)


class TestCodeDelegation(unittest.TestCase):
    """The doc's delegation target (issue #978) is real, importable code."""

    def setUp(self):
        from anvil.lib.skill_lib_loader import import_skill_lib_module

        ip_search_lib = _SKILL_ROOT.parents[0] / "ip-search" / "lib"
        self.prior_art_step = import_skill_lib_module(
            "ip-search", ip_search_lib, "prior_art_step"
        )

    def test_module_exposes_the_documented_api(self):
        for name in ("KNOB_KEY", "resolve_config", "run_step", "StepConfig"):
            self.assertTrue(hasattr(self.prior_art_step, name))
        self.assertEqual(self.prior_art_step.KNOB_KEY, "prior_art_search")

    def test_off_by_default_is_structural(self):
        """No knob ⇒ the runner is never even called."""

        def _exploding_runner(*_a, **_kw):
            raise AssertionError("runner must not be called when the knob is off")

        with tempfile.TemporaryDirectory() as tmp:
            thread = Path(tmp) / "acme-widget-prov"
            thread.mkdir()
            result = self.prior_art_step.run_step(thread, runner=_exploding_runner)
            self.assertEqual(result.state, self.prior_art_step.STATE_DISABLED)

    def test_shipped_config_shape_resolves_enabled(self):
        """#975's shipped `.anvil.json` shape still opts in after #978."""

        with tempfile.TemporaryDirectory() as tmp:
            thread = Path(tmp) / "acme-widget-prov"
            thread.mkdir()
            (thread / ".anvil.json").write_text(
                json.dumps(
                    {
                        "prior_art_search": {
                            "auto": True,
                            "corpus": "auto",
                            "max": 8,
                            "min_score": 1,
                        }
                    }
                ),
                encoding="utf-8",
            )
            cfg = self.prior_art_step.resolve_config(thread)
            self.assertTrue(cfg.auto)


if __name__ == "__main__":
    unittest.main()
