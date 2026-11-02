"""Tests for ``anvil/skills/essay/lib/claim_ledger.py`` (issue #1198).

Covers the acceptance criteria from #1198:

- A claim ledger row round-trips through the extended 8-column shape
  (``Claim | Kind | Source file | Line range | Anchor | Verified
  against | Verified at | Notes``).
- A legacy #597/#868 table (no ``Kind``/``Verified against``/
  ``Verified at`` columns) still parses -- every row reports ``kind =
  None`` and :data:`claim_ledger.VERIFICATION_NEVER`, never a defect
  (backward-compat, mirrors ``anvil.lib.provenance_anchor``'s
  ``STATUS_NO_ANCHOR`` posture for pre-#868 rows).
- ``unverified_rows`` surfaces every row with no ``Verified at`` --
  the never-checked-claim batching primitive Failure 3 (#888) needs.
- ``stale_corpus_rows`` reuses ``anvil.lib.provenance_anchor`` to
  detect a ``corpus``-kind row whose cited source drifted/vanished
  since it was last verified, and never evaluates a ``derived`` row.
- ``mark_verified`` mechanically stamps ``Verified against``/
  ``Verified at`` on named rows, leaves every other cell byte-identical,
  and no-ops on a legacy table with no such columns.
- ``anvil.lib.provenance_anchor.repoint_drifted_anchors`` (the #868
  mechanical repoint) runs unmodified against this module's extended
  ledger shape and only ever touches ``corpus``-kind rows (reuse, not
  duplication, of the content-addressed anchor pattern).
- ``summarize`` produces the roll-up counts fed to
  ``_progress.json.metadata.claim_ledger_summary``.
- The CLI ``check`` / ``mark-verified`` / ``new`` subcommands round-trip
  and use the documented exit codes.

This file is named ``test_essay_claim_ledger.py`` (not a generic
``test_claim_ledger.py``) to avoid the cross-skill pytest filename
collision documented in issue #58. The package carries an
``__init__.py`` for the same reason.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

_HERE = Path(__file__).resolve().parent
_REPO_ROOT = _HERE.parents[3]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from anvil.lib.provenance_anchor import repoint_drifted_anchors  # noqa: E402
from anvil.skills.essay.lib.claim_ledger import (  # noqa: E402
    KIND_CORPUS,
    KIND_DERIVED,
    VERIFICATION_CURRENT,
    VERIFICATION_NEVER,
    VERIFICATION_STALE,
    age_in_versions,
    check_claim_ledger,
    main,
    mark_verified,
    new_ledger_text,
    parse_claim_ledger,
    stale_corpus_rows,
    summarize,
    unverified_rows,
)

FULL_LEDGER = """# Claim ledger

| Claim | Kind | Source file | Line range | Anchor | Verified against | Verified at | Notes |
|---|---|---|---|---|---|---|---|
| "The factory burned down" | corpus | transcripts/nita3.txt | 9-9 | "the factory burned down in the summer of 1942" | essay-review v1 | 1 | verbatim recall |
| The repo has 42 open issues | derived | refs/repo-scan.md |  |  |  |  | count from linked repo |
| Campaign ran for six weeks | derived | refs/campaign-notes.md |  |  | essay-review v1 | 1 | inferred from dates |
"""

LEGACY_LEDGER = """# Claim provenance

| Claim | Source file | Line range | Notes |
|---|---|---|---|
| "The factory burned down" | transcripts/nita3.txt | 9-9 | verbatim recall |
"""


def _write(path: Path, content: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    return path


@pytest.fixture()
def corpus_dir(tmp_path: Path) -> Path:
    transcripts = tmp_path / "transcripts"
    transcripts.mkdir()
    lines = [f"filler line {i}" for i in range(1, 9)]
    text = "\n".join(lines) + "\nthe factory burned down in the summer of 1942\n"
    _write(transcripts / "nita3.txt", text)
    return tmp_path


class TestParsing:
    def test_full_shape_parses_all_columns(self, tmp_path: Path) -> None:
        path = _write(tmp_path / "provenance.md", FULL_LEDGER)
        ledger = parse_claim_ledger(path)
        assert len(ledger.rows) == 3

        corpus_row = ledger.rows[0]
        assert corpus_row.kind == KIND_CORPUS
        assert corpus_row.verified_against == "essay-review v1"
        assert corpus_row.verified_at == 1
        assert corpus_row.anchor == "the factory burned down in the summer of 1942"

        never_verified = ledger.rows[1]
        assert never_verified.kind == KIND_DERIVED
        assert never_verified.verified_at is None
        assert never_verified.verified_against is None

    def test_legacy_shape_parses_with_unset_kind_and_never_verified(
        self, tmp_path: Path
    ) -> None:
        path = _write(tmp_path / "provenance.md", LEGACY_LEDGER)
        ledger = parse_claim_ledger(path)
        assert len(ledger.rows) == 1
        row = ledger.rows[0]
        # Legacy table has no Kind/Verified columns at all.
        assert ledger.kind_col is None
        assert ledger.verified_at_col is None
        assert row.kind is None
        assert row.verified_at is None
        assert row.claim == '"The factory burned down"'

    def test_missing_file_returns_empty_ledger(self, tmp_path: Path) -> None:
        ledger = parse_claim_ledger(tmp_path / "does-not-exist.md")
        assert ledger.rows == []

    def test_unrecognized_kind_value_normalizes_to_none(self, tmp_path: Path) -> None:
        text = FULL_LEDGER.replace("| corpus |", "| bogus |")
        path = _write(tmp_path / "provenance.md", text)
        ledger = parse_claim_ledger(path)
        assert ledger.rows[0].kind is None


class TestVerificationQueries:
    def test_unverified_rows_finds_the_never_checked_claim(self, tmp_path: Path) -> None:
        path = _write(tmp_path / "provenance.md", FULL_LEDGER)
        ledger = parse_claim_ledger(path)
        unverified = unverified_rows(ledger.rows)
        assert len(unverified) == 1
        assert unverified[0].claim == "The repo has 42 open issues"

    def test_stale_corpus_rows_detects_drift_and_skips_derived(
        self, corpus_dir: Path
    ) -> None:
        # Insert a line above the cited passage so the Line range hint
        # goes stale while the anchor text survives -- the #868 drift
        # shape, now exercised through the extended ledger contract.
        corpus_file = corpus_dir / "transcripts" / "nita3.txt"
        corpus_file.write_text("INSERTED LINE\n" + corpus_file.read_text())

        path = _write(corpus_dir / "provenance.md", FULL_LEDGER)
        ledger = parse_claim_ledger(path)
        stale = stale_corpus_rows(ledger.rows, [corpus_dir])
        assert len(stale) == 1
        assert stale[0].kind == KIND_CORPUS
        # The derived rows are never evaluated for drift at all.
        assert all(r.kind != KIND_DERIVED for r in stale)

    def test_stale_corpus_rows_is_empty_when_nothing_drifted(
        self, corpus_dir: Path
    ) -> None:
        path = _write(corpus_dir / "provenance.md", FULL_LEDGER)
        ledger = parse_claim_ledger(path)
        assert stale_corpus_rows(ledger.rows, [corpus_dir]) == []

    def test_age_in_versions_computes_gap_and_none_when_unverified(
        self, tmp_path: Path
    ) -> None:
        path = _write(tmp_path / "provenance.md", FULL_LEDGER)
        ledger = parse_claim_ledger(path)
        verified_row = ledger.rows[0]
        never_row = ledger.rows[1]
        assert age_in_versions(verified_row, current_version=4) == 3
        assert age_in_versions(never_row, current_version=4) is None


class TestCheckClaimLedger:
    def test_reports_all_three_statuses(self, corpus_dir: Path) -> None:
        corpus_file = corpus_dir / "transcripts" / "nita3.txt"
        corpus_file.write_text("INSERTED LINE\n" + corpus_file.read_text())

        path = _write(corpus_dir / "provenance.md", FULL_LEDGER)
        report = check_claim_ledger(path, [corpus_dir], current_version=2)

        assert report["total_rows"] == 3
        assert report["counts"][VERIFICATION_STALE] == 1
        assert report["counts"][VERIFICATION_NEVER] == 1
        assert report["counts"][VERIFICATION_CURRENT] == 1
        # priority_lines batches BOTH stale and never-verified rows --
        # the fix for Failure 3's trickling: one pass sees everything
        # that needs attention, not just what changed this round.
        assert len(report["priority_lines"]) == 2

    def test_missing_ledger_reports_zero_rows_never_raises(self, tmp_path: Path) -> None:
        report = check_claim_ledger(tmp_path / "nope.md", [tmp_path])
        assert report["total_rows"] == 0
        assert report["priority_lines"] == []


class TestMarkVerified:
    def test_stamps_named_rows_leaves_others_untouched(self, tmp_path: Path) -> None:
        path = _write(tmp_path / "provenance.md", FULL_LEDGER)
        before = path.read_text()
        result = mark_verified(path, [6], "essay-review v2 (derived check)", 2)
        assert result["updated"] == [6]

        after = parse_claim_ledger(path)
        touched = [r for r in after.rows if r.line_no == 6][0]
        assert touched.verified_against == "essay-review v2 (derived check)"
        assert touched.verified_at == 2

        # Every other row's cells are byte-identical.
        untouched_before = before.splitlines()[4]
        untouched_after = path.read_text().splitlines()[4]
        assert untouched_before == untouched_after

    def test_noop_when_no_verified_columns_present(self, tmp_path: Path) -> None:
        path = _write(tmp_path / "provenance.md", LEGACY_LEDGER)
        before = path.read_text()
        result = mark_verified(path, [4], "essay-review v1", 1)
        assert result["updated"] == []
        assert path.read_text() == before

    def test_unmatched_line_numbers_are_ignored(self, tmp_path: Path) -> None:
        path = _write(tmp_path / "provenance.md", FULL_LEDGER)
        result = mark_verified(path, [999], "essay-review v2", 2)
        assert result["updated"] == []


class TestRepointReuse:
    def test_provenance_anchor_repoint_only_touches_corpus_rows(
        self, corpus_dir: Path
    ) -> None:
        """The #868 mechanical-repoint tool runs UNMODIFIED against
        this module's extended ledger shape (issue #1198's "reuse
        provenance_anchor.py's content-addressed anchor pattern"
        directive) -- it detects the drifted corpus row and repoints
        only its Line range cell, leaving the derived rows (which carry
        no Anchor) untouched."""
        corpus_file = corpus_dir / "transcripts" / "nita3.txt"
        corpus_file.write_text("INSERTED LINE\n" + corpus_file.read_text())

        path = _write(corpus_dir / "provenance.md", FULL_LEDGER)
        report = repoint_drifted_anchors(path, [corpus_dir])
        assert len(report["repointed"]) == 1
        assert report["repointed"][0]["row_line"] == 5

        ledger = parse_claim_ledger(path)
        corpus_row = ledger.rows[0]
        assert corpus_row.line_range_raw != "9-9"
        # The derived rows are byte-identical -- their Kind/Source/Notes
        # cells never touched by a tool that has no idea "Kind" exists.
        derived_rows = [r for r in ledger.rows if r.kind == KIND_DERIVED]
        assert len(derived_rows) == 2
        assert derived_rows[0].source_file == "refs/repo-scan.md"


class TestSummarize:
    def test_counts_by_kind_and_verification(self, tmp_path: Path) -> None:
        path = _write(tmp_path / "provenance.md", FULL_LEDGER)
        ledger = parse_claim_ledger(path)
        summary = summarize(ledger.rows)
        assert summary == {
            "total_claims": 3,
            "corpus": 1,
            "derived": 2,
            "unset_kind": 0,
            "verified": 2,
            "unverified": 1,
        }

    def test_empty_ledger_summarizes_to_zeros(self) -> None:
        summary = summarize([])
        assert summary["total_claims"] == 0
        assert summary["unverified"] == 0


class TestNewLedgerText:
    def test_skeleton_round_trips_to_zero_rows(self, tmp_path: Path) -> None:
        path = _write(tmp_path / "provenance.md", new_ledger_text())
        ledger = parse_claim_ledger(path)
        assert ledger.rows == []
        assert ledger.kind_col is not None
        assert ledger.verified_at_col is not None


class TestCli:
    def test_check_subcommand_exits_zero_and_prints_json(
        self, tmp_path: Path, capsys: pytest.CaptureFixture[str]
    ) -> None:
        path = _write(tmp_path / "provenance.md", FULL_LEDGER)
        rc = main(["check", str(path), str(tmp_path)])
        assert rc == 0
        payload = json.loads(capsys.readouterr().out)
        assert payload["total_rows"] == 3

    def test_mark_verified_subcommand(
        self, tmp_path: Path, capsys: pytest.CaptureFixture[str]
    ) -> None:
        path = _write(tmp_path / "provenance.md", FULL_LEDGER)
        rc = main(
            [
                "mark-verified",
                str(path),
                "--lines",
                "6",
                "--against",
                "essay-review v2",
                "--at",
                "2",
            ]
        )
        assert rc == 0
        payload = json.loads(capsys.readouterr().out)
        assert payload["updated"] == [6]

    def test_new_subcommand_prints_skeleton(
        self, capsys: pytest.CaptureFixture[str]
    ) -> None:
        rc = main(["new"])
        assert rc == 0
        out = capsys.readouterr().out
        assert "Claim" in out
        assert "Verified at" in out
