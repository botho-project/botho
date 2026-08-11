"""Regression test: the shipped minimal synthetic memoir example parses
(issue #740).

``anvil/skills/memoir/examples/grani-memoir-mini/`` is a **minimal
synthetic worked example** (NOT the full ``nitas-mama`` dogfood — that
vendored example is deferred to a follow-up issue per SKILL.md §Scope
guard, the same sequencing ``primer``/``spec`` used before their own
Phase-4 dogfood examples landed). It declares ``artifact_type: memoir``,
a top-level ``corpus:`` list (#597), and a ``voice.subjects:`` entry
(#598) — this test pins the vendored example against the strict loader,
the parsed ``corpus:`` resolution, and the #346 rubric stamps on all
three critic siblings (``rubric_id: "anvil-memoir-v1"``, ``rubric_total:
44``, audit-grade ``advance_threshold: 39``).

Per the #58 packaging convention this filename
(``test_memoir_example_brief_parses.py``) is unique across the
``anvil/skills/*/tests/`` tree.

Runs under either ``pytest anvil/skills/memoir/tests/`` or
``python -m unittest discover anvil/skills/memoir/tests/``.
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path

from anvil.lib.project_brief import (
    ArtifactType,
    load_project_brief_strict,
    resolve_corpus_dirs,
    resolve_subject_voice_docs,
)


_PROJECT = "grani-memoir-mini"
_SLUG = "00-introduction"
_EXAMPLE_DIR = Path(__file__).resolve().parent.parent / "examples" / _PROJECT
_THREAD_DIR = _EXAMPLE_DIR / _SLUG
_V1 = _THREAD_DIR / f"{_SLUG}.1"
_REVIEW = _THREAD_DIR / f"{_SLUG}.1.review"
_AUDIT = _THREAD_DIR / f"{_SLUG}.1.audit"
_CORPUS_AUDIT = _THREAD_DIR / f"{_SLUG}.1.corpus-audit"


class TestShippedMemoirExampleBriefParses(unittest.TestCase):
    """The memoir skill's minimal worked example must parse under the
    strict loader."""

    def test_example_dir_ships_a_brief(self) -> None:
        self.assertTrue(
            (_EXAMPLE_DIR / "BRIEF.md").is_file(),
            f"expected shipped example BRIEF at {_EXAMPLE_DIR / 'BRIEF.md'}",
        )

    def test_shipped_brief_parses_strict(self) -> None:
        brief = load_project_brief_strict(_EXAMPLE_DIR)
        self.assertEqual(brief.project, _PROJECT)
        slugs = [d.slug for d in brief.documents]
        self.assertIn(_SLUG, slugs)

    def test_shipped_brief_declares_memoir_artifact_type(self) -> None:
        brief = load_project_brief_strict(_EXAMPLE_DIR)
        doc = next(d for d in brief.documents if d.slug == _SLUG)
        self.assertEqual(doc.artifact_type, ArtifactType.MEMOIR)

    def test_corpus_tier_resolves_both_roots(self) -> None:
        # corpus: is declared once at the project level (SKILL.md §Dual-
        # corpus provenance) and resolves both roots for real (this
        # example is genuinely self-contained, unlike spec's illustrative-
        # only code_ref).
        resolved = resolve_corpus_dirs(_EXAMPLE_DIR, consumer_root=_EXAMPLE_DIR)
        self.assertEqual(len(resolved), 2)
        self.assertFalse(any(r.missing for r in resolved))
        declared = {r.declared for r in resolved}
        self.assertEqual(declared, {"transcripts/", "letters/"})

    def test_subject_voice_tier_resolves(self) -> None:
        resolved = resolve_subject_voice_docs(
            _EXAMPLE_DIR, consumer_root=_EXAMPLE_DIR
        )
        self.assertEqual(len(resolved), 1)
        self.assertEqual(resolved[0].name, "grani")
        self.assertFalse(resolved[0].corpus.missing)

    def test_body_file_echoes_the_slug(self) -> None:
        body = _V1 / f"{_SLUG}.tex"
        self.assertTrue(
            body.is_file(),
            f"expected slug-echo LaTeX body at {body}",
        )

    def test_provenance_map_present(self) -> None:
        provenance = _V1 / "provenance.md"
        self.assertTrue(provenance.is_file())
        text = provenance.read_text(encoding="utf-8")
        # 5 data rows (+ 1 header separator row) in the pipe table.
        rows = [
            line
            for line in text.splitlines()
            if line.startswith("|") and "---" not in line and "Claim" not in line
        ]
        self.assertEqual(len(rows), 5)

    def test_all_three_critic_siblings_exist(self) -> None:
        # corpus: is active for this example, so ALL THREE siblings must
        # exist (SKILL.md §Dual-corpus provenance; memoir-audit.md §What
        # memoir-audit does NOT do — never skips the corpus-audit sibling
        # when the tier is active).
        for sibling in (_REVIEW, _AUDIT, _CORPUS_AUDIT):
            self.assertTrue(
                (sibling / "verdict.md").is_file(),
                f"expected verdict.md in {sibling}",
            )
            self.assertTrue(
                (sibling / "_meta.json").is_file(),
                f"expected _meta.json in {sibling}",
            )
        self.assertTrue((_REVIEW / "scoring.md").is_file())
        self.assertTrue((_AUDIT / "findings.md").is_file())
        self.assertTrue((_CORPUS_AUDIT / "findings.md").is_file())

    def test_critic_meta_carries_rubric_stamps(self) -> None:
        for sibling in (_REVIEW, _AUDIT, _CORPUS_AUDIT):
            meta = json.loads((sibling / "_meta.json").read_text())
            self.assertEqual(meta["scorecard_kind"], "human-verdict")
            self.assertEqual(meta["rubric_id"], "anvil-memoir-v1")
            self.assertEqual(meta["rubric_total"], 44)
            self.assertEqual(meta["advance_threshold"], 39)

    def test_review_advances_and_both_audits_are_clean(self) -> None:
        review_progress = json.loads((_REVIEW / "_progress.json").read_text())
        self.assertTrue(review_progress["metadata"]["advance"])
        self.assertGreaterEqual(review_progress["metadata"]["total"], 39)
        audit_progress = json.loads((_AUDIT / "_progress.json").read_text())
        self.assertTrue(audit_progress["metadata"]["audit_clean"])
        corpus_audit_progress = json.loads(
            (_CORPUS_AUDIT / "_progress.json").read_text()
        )
        self.assertTrue(corpus_audit_progress["metadata"]["audit_clean"])

    def test_corpus_audit_provenance_summary_accounts_for_all_claims(self) -> None:
        summary = json.loads((_CORPUS_AUDIT / "_progress.json").read_text())[
            "metadata"
        ]["provenance_summary"]
        counted = (
            summary["verified"]
            + summary["paraphrase_ok"]
            + summary["mismatch"]
            + summary["not_found"]
            + summary["fabricated"]
        )
        self.assertEqual(counted, summary["total_claims"])
        self.assertEqual(summary["fabricated"], 0)

    def test_no_pdf_vendored(self) -> None:
        # memoir's canonical output is the LaTeX source (SKILL.md §Output
        # format); no compiled PDF is vendored for this minimal example.
        pdfs = list(_EXAMPLE_DIR.rglob("*.pdf"))
        self.assertEqual(pdfs, [], f"no compiled PDF may be vendored: {pdfs}")

    def test_example_stays_within_size_envelope(self) -> None:
        total = sum(
            p.stat().st_size for p in _EXAMPLE_DIR.rglob("*") if p.is_file()
        )
        self.assertLess(
            total,
            100 * 1024,
            f"vendored minimal memoir example is {total // 1024} KB — expected "
            f"< 100 KB (it is deliberately synthetic-minimal, not a dogfood)",
        )


if __name__ == "__main__":
    unittest.main()
