"""Cross-version claim ledger for anvil:essay (issue #1198).

The problem
-----------

Failure 3 from issue #888 ("review loop has no lookahead and no
cross-version claim ledger"): a review pass had no record of what had
already been checked, so effort went to *re-verifying* claims that were
unchanged since an earlier version while *never-checked* claims sat
untouched for several rounds -- the two critical flags in #888's
original report both landed on clauses "carried untouched since `.1`
that no pass had ever verified," surfaced only at iterations 4 and 5.
Findings that could have all been caught in one pass instead trickled
across many small revisions (`.5` was a restructure plus four
corrections; `.6` was three more hunks).

The existing local-corpus claim-provenance contract
(``anvil/lib/snippets/provenance.md``, issue #597) already solves the
narrower case of a claim attributed to a declared ``corpus:`` -- but it
does not cover claims that are not corpus-attributed at all: a record
count derived from a linked repo, a campaign count read off a `refs/`
file, or a prose-logic conclusion a prior review pass already checked
by argument rather than by opening a source file. Those claims were
silently re-derived every pass (waste) or never checked at all (the
concrete #888 failure).

Design decision (issue #1198)
------------------------------

Two open questions, both resolved explicitly rather than by default:

1. **Placement: skill-local.** This module lives under
   ``anvil/skills/essay/lib/``, not ``anvil/lib/``, per the "skill-local
   first, promote on a second consumer" rule (``CLAUDE.md`` section
   "Working on this repo"). ``anvil:essay`` is the only reported
   consumer of this failure mode today. The row shape below is
   deliberately kept compatible with the framework snippet's contract
   (same column names, same anchor semantics) so a future promotion to
   ``anvil/lib/`` would be a mechanical move, not a format migration.

2. **Relationship to the corpus-provenance ledger: extend, don't
   parallel.** Rather than a second, freestanding ledger file, this
   module treats ``<thread>.{N}/provenance.md`` as ONE ledger covering
   BOTH claim kinds:

   - ``Kind: corpus`` rows are exactly today's #597 rows -- a
     ``Source file`` + ``Line range`` + optional #868 ``Anchor``
     resolved against a declared ``corpus:`` directory. Parsing and
     anchor-drift detection for these rows is delegated directly to
     ``anvil.lib.provenance_anchor`` (reused, not reimplemented) via
     :func:`stale_corpus_rows`.
   - ``Kind: derived`` rows are the new case #888 names: a claim
     grounded in ``<thread>/refs/``, shared ``research/``, an external
     repo, or a prior review's prose-logic judgment. ``Source file``
     holds a free-text reference (a path, a repo+commit, or a pointer
     like ``"essay-review v2, comments.md"``) rather than a
     corpus-resolvable path -- :mod:`anvil.lib.provenance_anchor` is
     never run against these rows.

   Both kinds share two new columns this module adds to the row shape:
   ``Verified against`` (what evidence check last validated the row --
   e.g. ``"essay-review v2 (corpus back-check)"``) and ``Verified at``
   (the version number ``N`` at which that check happened). A review
   pass's job (``unverified_rows`` / ``stale_corpus_rows``) becomes
   "check what's unverified or whose source moved," not "re-derive
   everything" -- Failure 3's fix.

   The corpus tier's own activation rule (``corpus:`` BRIEF key,
   ``anvil/lib/snippets/provenance.md`` Section 1) is UNCHANGED -- this
   module never activates or deactivates it. What changes is that
   essay's ``provenance.md`` may now ALSO exist, or carry ADDITIONAL
   rows, when the drafter/reviser has ``derived``-kind claims to track
   even though ``corpus:`` is inactive. See
   ``anvil/skills/essay/commands/essay-draft.md`` section "Claim
   ledger" for the activation rule (write iff there is at least one
   checkable claim; corpus tier stays byte-identical when inactive) and
   ``anvil/skills/essay/SKILL.md`` section "Claim ledger" for the full
   contract.

What the killed ``_convictions.md`` primitive (#142/#225/#226/#227/#228)
got wrong, and how this design avoids it
--------------------------------------------------------------------------

``_convictions.md`` (issue #142, shipped by #155, reverted by #227) was
a reviser-written per-version file recording "settled" editorial
positions, consumed by "the next reviser." Two structural defects
killed it (canary evidence in #225/#226):

- **#225 -- write-only by construction.** The named consumer ("the next
  reviser at version N+1") only exists when the thread does NOT
  advance. A cleanly-advancing thread -- the modal, intended outcome --
  produces a file with no reader, ever. The read and write triggers
  were coupled to ``verdict.advance``, and the coupling made the file
  dead-letter in the common case.
- **#226 -- wrong trigger.** The write trigger (`Resolution: declined`
  rows in ``changelog.md``) selected against the primitive's own most
  valuable cases -- the load-bearing convictions lived in `addressed`
  rows where the *addressing judgment itself* was the held position,
  not in the stylistic nits that got explicitly declined.

This design avoids both by construction, not by discipline:

- **No coupling to ``verdict.advance``.** Every ``essay-review`` pass
  reads :func:`unverified_rows` / :func:`stale_corpus_rows` REGARDLESS
  of whether the prior version advanced -- the ledger's read trigger is
  "does an unverified or possibly-stale row exist," a property of the
  ledger itself, not of the review's outcome. Every ``essay-revise``
  pass copies the ledger forward REGARDLESS of whether critical flags
  fired. There is no verdict-gated branch anywhere in this module or in
  the read/write steps of ``essay-review.md`` / ``essay-revise.md`` --
  the failure mode #225 exploited (a consumer that only exists on one
  branch of the verdict) cannot recur because no such branch exists.
- **The trigger is claim-shaped, not changelog-shaped.** ``verified_at``
  is stamped on a *claim row* the moment a review pass actually checks
  it -- never inferred from whether a critic note was `declined` vs.
  `addressed` in prose. #226's defect was specific to filtering
  ``changelog.md`` resolution tags; this primitive has no dependency on
  that vocabulary at all, so the same misclassification cannot recur.
- **A named consumer that fires on EVERY pass, not a conditional one.**
  #142's file was "for the next reviser," a role that only sometimes
  exists. This ledger's consumer is "the next ``essay-review`` pass,"
  which by the state machine ALWAYS runs on every reviewed version --
  whether the prior version advanced determines whether ``essay-revise``
  produces a NEXT version at all (see ``essay-revise.md`` step 2's
  READY short-circuit), but every version that IS reviewed gets its
  claims checked against this ledger.

Row shape
---------

::

    | Claim | Kind | Source file | Line range | Anchor | Verified against | Verified at | Notes |

- **Claim** -- the attributed quote or factual assertion, exactly as
  ``anvil/lib/snippets/provenance.md`` Section 2 already documents.
- **Kind** -- ``corpus`` or ``derived``. Blank / unrecognized on a
  pre-#1198 row -> treated as unset (never a defect -- the same
  "unknown is honest" posture as
  ``anvil.lib.provenance_anchor.STATUS_NO_ANCHOR``).
- **Source file** -- for ``corpus`` rows, a path relative to a declared
  corpus dir (unchanged from #597); for ``derived`` rows, a free-text
  evidentiary reference (a ``refs/`` path, a repo + commit, or a
  pointer into a prior review's prose judgment).
- **Line range** -- unchanged #597 hint column; blank for most
  ``derived`` rows.
- **Anchor** -- unchanged #868 content-addressed identity; populated
  for ``corpus`` rows and for any ``derived`` row whose reference is
  itself a quotable local file passage.
- **Verified against** (new, #1198) -- free text naming the check that
  last validated this row, e.g. ``"essay-review v2 (corpus
  back-check)"`` or ``"essay-review v3 (numeric pass)"``.
- **Verified at** (new, #1198) -- the essay version number ``N`` at
  which that check ran. ``None``/blank -> never verified (the row that
  #888's two original critical flags both landed on).
- **Notes** -- unchanged drafter characterization column.

Composition with ``_progress.json`` (issue #350's checkpointing)
------------------------------------------------------------------

This module does not duplicate ``_progress.json``'s phase-state
tracking. It reads/writes only the ledger file itself; the caller
(``essay-review.md`` / ``essay-revise.md``) is responsible for
recording a roll-up (:func:`summarize`) under
``_progress.json.metadata.claim_ledger_summary``, mirroring the
existing ``metadata.provenance_summary`` convention
(``anvil/lib/snippets/provenance.md`` Section 7) -- a read-only summary
for a human/orchestrator scanning progress state, never a second
source of truth for verification status (the file itself is that).

Composition with ``anvil/lib/critics.py`` / ``anvil/lib/convergence.py``
--------------------------------------------------------------------------

Neither framework module is modified by this issue, deliberately:

- **``critics.py``** aggregates ``Finding``/``CriticalFlag`` objects a
  critic already wrote into ``verdict.md``/``comments.md``/
  ``_review.json``. This module feeds that machinery the SAME way
  ``provenance_anchor.py`` already does: unverified/stale rows become
  ordinary ``major``/``blocker`` findings in ``comments.md`` (see
  ``essay-review.md`` step 4c), routed through the existing
  ``compute_verdict`` aggregation unmodified. No new ``Finding`` field
  and no new critical-flag type is needed for the ledger itself (a
  fabrication CAUGHT via the ledger still raises the existing
  ``fabricated_fact`` etc. vocabulary from
  ``anvil/lib/snippets/provenance.md`` Section 6).
- **``convergence.py``** decides when the review/revise LOOP stops
  (``THRESHOLD_MET`` / ``STALLED`` / ``MAX_ITERATIONS`` / critical-flag
  short-circuit) from a pure score history. This ledger changes WHAT a
  review pass covers each iteration (batch, don't trickle), not WHEN
  the loop terminates -- it is claim-shaped bookkeeping, not a score
  signal, so it deliberately introduces no new ``Verdict`` branch and
  no change to ``decide_termination``'s resolution order. A thread that
  converges with an unverified claim backlog is still caught by the
  existing machinery: an unverified/stale row is a ``major``/``blocker``
  finding like any other, so it depresses the rubric total or raises a
  critical flag exactly as a hand-found defect would -- the ledger only
  changes how reliably that finding is produced each pass.

CLI entry-point
----------------

``python -m anvil.skills.essay.lib.claim_ledger check <provenance.md> [<corpus_root> ...]``
    Reports every row's verification status as JSON -- ``NEVER_VERIFIED``,
    ``NEEDS_REVERIFY`` (a ``corpus`` row whose anchor drifted/vanished,
    detected via ``anvil.lib.provenance_anchor``), or
    ``VERIFIED_CURRENT``. Exit code is always ``0`` -- advisory, like
    ``provenance_anchor``'s CLI.

``python -m anvil.skills.essay.lib.claim_ledger mark-verified <provenance.md> --lines L1,L2,... --against "<text>" --at <N>``
    Mechanically stamps ``Verified against`` / ``Verified at`` on the
    given row line numbers. This is the ONLY mutation this module
    performs on a ledger row -- ``Claim`` / ``Kind`` / ``Source file`` /
    ``Anchor`` / ``Notes`` are never touched. Atomic via
    :func:`anvil.lib.atomic_write.atomic_write_text`.

``python -m anvil.skills.essay.lib.claim_ledger new``
    Prints an empty ledger skeleton (header + separator row only) for
    ``essay-draft`` to seed a fresh ``provenance.md``.
"""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence

from anvil.lib.atomic_write import atomic_write_text
from anvil.lib.provenance_anchor import (
    STATUS_DRIFTED,
    STATUS_FILE_NOT_FOUND,
    STATUS_NOT_FOUND,
    ProvenanceRow,
    _parse_line_ranges,
    resolve_anchor,
    resolve_source_file,
)

# ---------------------------------------------------------------------------
# Claim-kind vocabulary
# ---------------------------------------------------------------------------

KIND_CORPUS = "corpus"
"""A claim attributed to a declared ``corpus:`` directory -- the #597
tier, unchanged. Anchor-drift-checkable via ``anvil.lib.provenance_anchor``."""

KIND_DERIVED = "derived"
"""A claim NOT attributed to a declared corpus -- a count from a linked
repo, a `refs/` fact, or a prose-logic conclusion a prior pass already
checked. Never anchor-drift-checked (no corpus root to resolve a
free-text reference against); staleness for this kind is a judgment
call left to the reviewer, using ``verified_at``'s age as a hint (see
:func:`age_in_versions`)."""

_KINDS = (KIND_CORPUS, KIND_DERIVED)

# Verification-status vocabulary returned by :func:`check_claim_ledger`.

VERIFICATION_NEVER = "NEVER_VERIFIED"
"""``Verified at`` is unset -- this row has never been checked by any
review pass. The #888 failure mode: an inherited, never-touched claim
must not silently survive multiple rounds unverified."""

VERIFICATION_STALE = "NEEDS_REVERIFY"
"""A ``corpus``-kind row whose cited source drifted or vanished
(``anvil.lib.provenance_anchor`` reported ``DRIFTED``/``NOT_FOUND``/
``FILE_NOT_FOUND``) since it was last verified -- the prior
verification no longer speaks for the current source location/content."""

VERIFICATION_CURRENT = "VERIFIED_CURRENT"
"""``Verified at`` is set and, for a ``corpus`` row, its anchor still
resolves cleanly (or the row predates the anchor column). A review pass
may skip this row unless something else about the claim changed."""

DEFAULT_HEADER = [
    "Claim",
    "Kind",
    "Source file",
    "Line range",
    "Anchor",
    "Verified against",
    "Verified at",
    "Notes",
]


# ---------------------------------------------------------------------------
# Table parsing (mirrors ``anvil.lib.provenance_anchor``'s header-name-based
# column detection -- issue #868 -- extended with the three new columns).
# ---------------------------------------------------------------------------


def _split_row(line: str) -> List[str]:
    stripped = line.strip()
    if stripped.startswith("|"):
        stripped = stripped[1:]
    if stripped.endswith("|"):
        stripped = stripped[:-1]
    return [cell.strip() for cell in stripped.split("|")]


def _is_separator_row(cells: Sequence[str]) -> bool:
    return bool(cells) and all(re.fullmatch(r":?-{2,}:?", c.strip()) for c in cells)


def _find_col(lowered_header: Sequence[str], *names: str) -> Optional[int]:
    for i, cell in enumerate(lowered_header):
        if any(name in cell for name in names):
            return i
    return None


@dataclass
class ClaimRow:
    """One parsed claim-ledger row."""

    line_no: int
    claim: str
    kind: Optional[str]
    source_file: str
    line_range_raw: str
    anchor: Optional[str]
    verified_against: Optional[str]
    verified_at: Optional[int]
    notes: str

    kind_col: Optional[int] = None
    source_col: Optional[int] = None
    line_range_col: Optional[int] = None
    anchor_col: Optional[int] = None
    verified_against_col: Optional[int] = None
    verified_at_col: Optional[int] = None
    notes_col: Optional[int] = None

    def to_provenance_row(self) -> ProvenanceRow:
        """Adapt this row to ``anvil.lib.provenance_anchor.ProvenanceRow``
        so :func:`anvil.lib.provenance_anchor.resolve_anchor` /
        :func:`anvil.lib.provenance_anchor.resolve_source_file` can be
        reused directly rather than reimplemented. Only meaningful for
        :data:`KIND_CORPUS` rows -- callers MUST filter by kind first
        (see :func:`stale_corpus_rows`)."""
        hints = _parse_line_ranges(self.line_range_raw)
        return ProvenanceRow(
            line_no=self.line_no,
            claim=self.claim,
            source_file=self.source_file,
            line_range_raw=self.line_range_raw,
            line_range=hints[0] if hints else None,
            anchor=self.anchor,
            notes=self.notes,
            line_range_hints=hints,
        )

    def to_json(self) -> Dict[str, Any]:
        return {
            "line_no": self.line_no,
            "claim": self.claim,
            "kind": self.kind,
            "source_file": self.source_file,
            "line_range": self.line_range_raw,
            "anchor": self.anchor,
            "verified_against": self.verified_against,
            "verified_at": self.verified_at,
            "notes": self.notes,
        }


@dataclass
class ParsedClaimLedger:
    """A parsed claim-ledger file (single conforming table -- essay
    threads carry one ledger, unlike the multi-table memoir-book case
    ``anvil.lib.provenance_anchor`` handles)."""

    header: List[str]
    rows: List[ClaimRow]
    lines: List[str]
    kind_col: Optional[int] = None
    source_col: Optional[int] = None
    line_range_col: Optional[int] = None
    anchor_col: Optional[int] = None
    verified_against_col: Optional[int] = None
    verified_at_col: Optional[int] = None
    notes_col: Optional[int] = None


def _normalize_kind(raw: str) -> Optional[str]:
    v = raw.strip().lower()
    return v if v in _KINDS else None


def _normalize_verified_at(raw: str) -> Optional[int]:
    v = raw.strip()
    if not v:
        return None
    try:
        return int(v)
    except ValueError:
        return None


def parse_claim_ledger(path: Path) -> ParsedClaimLedger:
    """Parse a claim-ledger ``provenance.md`` file.

    Tolerates the legacy #597/#868 4- or 5-column shape (no ``Kind`` /
    ``Verified against`` / ``Verified at`` columns at all -- every row
    parses with ``kind=None``, ``verified_at=None``, i.e.
    :data:`VERIFICATION_NEVER`) as well as the full #1198 8-column
    shape. Column identity is derived from the header row by name, not
    position (the same discipline as
    ``anvil.lib.provenance_anchor.parse_provenance_table``), so a
    consumer-reordered table still parses correctly.

    Returns an empty ledger (``rows=[]``) for a missing/empty file or a
    file with no conforming ``Claim``/``Source`` header -- never
    raises.
    """
    if not Path(path).is_file():
        return ParsedClaimLedger(header=[], rows=[], lines=[])

    lines = Path(path).read_text(encoding="utf-8").splitlines()
    n = len(lines)

    header: List[str] = []
    kind_col = source_col = line_range_col = anchor_col = None
    verified_against_col = verified_at_col = notes_col = None
    claim_col: Optional[int] = None
    rows: List[ClaimRow] = []

    idx = 0
    found_header = False
    while idx < n:
        line = lines[idx]
        if not line.strip().startswith("|"):
            idx += 1
            continue

        cells = _split_row(line)
        next_line = lines[idx + 1] if idx + 1 < n else ""
        next_is_separator = next_line.strip().startswith("|") and _is_separator_row(
            _split_row(next_line)
        )

        if not found_header:
            if not next_is_separator:
                idx += 1
                continue
            lowered = [c.lower() for c in cells]
            if "claim" not in lowered or not any("source" in c for c in lowered):
                idx += 2
                continue
            header = cells
            claim_col = _find_col(lowered, "claim")
            kind_col = _find_col(lowered, "kind")
            source_col = _find_col(lowered, "source")
            line_range_col = _find_col(lowered, "line range", "line-range")
            anchor_col = _find_col(lowered, "anchor")
            verified_against_col = _find_col(lowered, "verified against")
            verified_at_col = _find_col(lowered, "verified at")
            notes_col = _find_col(lowered, "notes")
            found_header = True
            idx += 2
            continue

        if _is_separator_row(cells) or len(cells) < 2:
            idx += 1
            continue

        def _cell(col: Optional[int]) -> str:
            if col is None or col >= len(cells):
                return ""
            return cells[col]

        anchor_cell = _cell(anchor_col) if anchor_col is not None else None
        anchor = anchor_cell.strip("\"'“” ") if anchor_cell else None
        if anchor is not None and not anchor:
            anchor = None

        verified_against = _cell(verified_against_col).strip() or None
        verified_at = _normalize_verified_at(_cell(verified_at_col))

        rows.append(
            ClaimRow(
                line_no=idx + 1,
                claim=_cell(claim_col),
                kind=_normalize_kind(_cell(kind_col)) if kind_col is not None else None,
                source_file=_cell(source_col),
                line_range_raw=_cell(line_range_col),
                anchor=anchor,
                verified_against=verified_against,
                verified_at=verified_at,
                notes=_cell(notes_col),
                kind_col=kind_col,
                source_col=source_col,
                line_range_col=line_range_col,
                anchor_col=anchor_col,
                verified_against_col=verified_against_col,
                verified_at_col=verified_at_col,
                notes_col=notes_col,
            )
        )
        idx += 1

    return ParsedClaimLedger(
        header=header,
        rows=rows,
        lines=lines,
        kind_col=kind_col,
        source_col=source_col,
        line_range_col=line_range_col,
        anchor_col=anchor_col,
        verified_against_col=verified_against_col,
        verified_at_col=verified_at_col,
        notes_col=notes_col,
    )


# ---------------------------------------------------------------------------
# Verification queries -- the batching primitive Failure 3 needs
# ---------------------------------------------------------------------------


def unverified_rows(rows: Sequence[ClaimRow]) -> List[ClaimRow]:
    """Rows with no ``Verified at`` at all -- never checked by any pass.
    The #888 failure: an inherited claim must not survive silently."""
    return [r for r in rows if r.verified_at is None]


def stale_corpus_rows(
    rows: Sequence[ClaimRow], corpus_roots: Sequence[Path]
) -> List[ClaimRow]:
    """``corpus``-kind rows whose cited source drifted, vanished, or is
    no longer resolvable, per ``anvil.lib.provenance_anchor`` -- i.e.
    rows whose PRIOR verification no longer speaks for the current
    source. Never run against ``derived`` rows (no corpus root to
    resolve a free-text reference against)."""
    stale: List[ClaimRow] = []
    for row in rows:
        if row.kind != KIND_CORPUS:
            continue
        pa_row = row.to_provenance_row()
        file_path = resolve_source_file(row.source_file, corpus_roots)
        resolution = resolve_anchor(file_path, pa_row)
        if resolution.status in (STATUS_DRIFTED, STATUS_NOT_FOUND, STATUS_FILE_NOT_FOUND):
            stale.append(row)
    return stale


def age_in_versions(row: ClaimRow, current_version: int) -> Optional[int]:
    """How many versions old this row's verification is (``current_version
    - verified_at``), or ``None`` if never verified. A judgment hint for
    ``derived`` rows, which have no mechanical drift check -- a large
    age is a reasonable prompt to re-derive by hand even absent a
    detected signal."""
    if row.verified_at is None:
        return None
    return max(0, current_version - row.verified_at)


def check_claim_ledger(
    path: Path,
    corpus_roots: Sequence[Path],
    current_version: Optional[int] = None,
) -> Dict[str, Any]:
    """Report every row's verification status as JSON. Advisory only --
    never mutates the file, never raises for a missing ledger (an
    absent file reports zero rows, the same "nothing to track" posture
    as an inactive corpus tier)."""
    ledger = parse_claim_ledger(path)
    stale = {row.line_no for row in stale_corpus_rows(ledger.rows, corpus_roots)}

    reports = []
    counts = {VERIFICATION_NEVER: 0, VERIFICATION_STALE: 0, VERIFICATION_CURRENT: 0}
    for row in ledger.rows:
        if row.verified_at is None:
            status = VERIFICATION_NEVER
        elif row.line_no in stale:
            status = VERIFICATION_STALE
        else:
            status = VERIFICATION_CURRENT
        counts[status] += 1
        entry = row.to_json()
        entry["status"] = status
        if current_version is not None:
            entry["age_in_versions"] = age_in_versions(row, current_version)
        reports.append(entry)

    priority_lines = sorted(
        r.line_no for r in ledger.rows if r.line_no in stale or r.verified_at is None
    )

    return {
        "provenance_path": str(path),
        "total_rows": len(ledger.rows),
        "counts": counts,
        "priority_lines": priority_lines,
        "rows": reports,
    }


# ---------------------------------------------------------------------------
# Mechanical verification stamping -- the ONLY mutation this module makes
# ---------------------------------------------------------------------------


def _rewrite_cell(line: str, col_index: int, new_value: str) -> str:
    """Rewrite the ``col_index``-th cell of a markdown table row,
    preserving every other cell and the row's leading/trailing pipe
    style -- mirrors
    ``anvil.lib.provenance_anchor._rewrite_line_range_cell``, applied to
    an arbitrary column rather than only ``Line range``."""
    stripped = line.strip()
    leading = "|" if stripped.startswith("|") else ""
    trailing = "|" if stripped.endswith("|") else ""
    body = stripped
    if leading:
        body = body[1:]
    if trailing and body.endswith("|"):
        body = body[:-1]
    cells = body.split("|")
    if col_index >= len(cells):
        return line
    cells[col_index] = f" {new_value} "
    return leading + "|".join(cells) + trailing


def mark_verified(
    path: Path,
    line_nos: Sequence[int],
    verified_against: str,
    verified_at: int,
) -> Dict[str, Any]:
    """Mechanically stamp ``Verified against`` / ``Verified at`` on the
    given 1-based row line numbers. Never touches ``Claim`` / ``Kind`` /
    ``Source file`` / ``Line range`` / ``Anchor`` / ``Notes`` -- the
    same boundary discipline as
    ``anvil.lib.provenance_anchor.repoint_drifted_anchors``: this
    records that a check happened, it never fabricates or edits the
    claim itself. A no-op (``updated: []``) when the ledger has no
    ``Verified against``/``Verified at`` column at all (a legacy table --
    the caller must add the columns via a normal markdown edit first, a
    one-time migration exactly like #868's ``Anchor`` column
    backward-compatibility posture) or when none of ``line_nos`` match a
    parsed row. Atomic write via
    :func:`anvil.lib.atomic_write.atomic_write_text`."""
    ledger = parse_claim_ledger(path)
    if ledger.verified_against_col is None or ledger.verified_at_col is None:
        return {
            "provenance_path": str(path),
            "updated": [],
            "detail": (
                "ledger has no 'Verified against'/'Verified at' column -- "
                "nothing stamped. Add both columns to the header (a "
                "one-time migration) before calling mark-verified."
            ),
        }

    by_line = {row.line_no: row for row in ledger.rows}
    new_lines = list(ledger.lines)
    updated: List[int] = []
    for line_no in line_nos:
        row = by_line.get(line_no)
        if row is None:
            continue
        idx = line_no - 1
        new_lines[idx] = _rewrite_cell(
            new_lines[idx], ledger.verified_against_col, verified_against
        )
        new_lines[idx] = _rewrite_cell(
            new_lines[idx], ledger.verified_at_col, str(verified_at)
        )
        updated.append(line_no)

    if updated:
        atomic_write_text(Path(path), "\n".join(new_lines) + "\n")

    return {
        "provenance_path": str(path),
        "updated": updated,
        "verified_against": verified_against,
        "verified_at": verified_at,
    }


# ---------------------------------------------------------------------------
# Summary -- for `_progress.json.metadata.claim_ledger_summary`
# ---------------------------------------------------------------------------


def summarize(rows: Sequence[ClaimRow]) -> Dict[str, Any]:
    """Roll-up counts for ``_progress.json.metadata.claim_ledger_summary``
    (mirrors the existing ``metadata.provenance_summary`` convention,
    ``anvil/lib/snippets/provenance.md`` Section 7) -- a read-only
    breadcrumb, never a second source of truth (the ledger file itself
    is that)."""
    total = len(rows)
    by_kind = {KIND_CORPUS: 0, KIND_DERIVED: 0, "unset": 0}
    verified = 0
    for row in rows:
        by_kind[row.kind if row.kind in _KINDS else "unset"] += 1
        if row.verified_at is not None:
            verified += 1
    return {
        "total_claims": total,
        "corpus": by_kind[KIND_CORPUS],
        "derived": by_kind[KIND_DERIVED],
        "unset_kind": by_kind["unset"],
        "verified": verified,
        "unverified": total - verified,
    }


def new_ledger_text(header: Optional[Sequence[str]] = None) -> str:
    """An empty ledger skeleton (header + separator row only), for
    ``essay-draft`` to seed a fresh ``<thread>.1/provenance.md``."""
    cols = list(header) if header is not None else list(DEFAULT_HEADER)
    head = "| " + " | ".join(cols) + " |"
    sep = "|" + "|".join(["---"] * len(cols)) + "|"
    return f"# Claim ledger\n\n{head}\n{sep}\n"


# ---------------------------------------------------------------------------
# CLI entry-point
# ---------------------------------------------------------------------------


def _build_cli_parser():
    import argparse

    p = argparse.ArgumentParser(
        prog="python -m anvil.skills.essay.lib.claim_ledger",
        description=(
            "Cross-version claim ledger for anvil:essay (issue #1198). "
            "'check' reports verification status; 'mark-verified' "
            "mechanically stamps rows a review pass just checked; 'new' "
            "prints an empty ledger skeleton."
        ),
    )
    sub = p.add_subparsers(dest="command", required=True)

    check_p = sub.add_parser(
        "check", help="Report every row's verification status as JSON."
    )
    check_p.add_argument("provenance_path", help="Path to the claim ledger.")
    check_p.add_argument(
        "corpus_roots",
        nargs="*",
        help="Resolved corpus root dir(s), for corpus-kind anchor-drift checking.",
    )
    check_p.add_argument(
        "--current-version",
        type=int,
        default=None,
        help="Current essay version number, for age_in_versions hints.",
    )

    mark_p = sub.add_parser(
        "mark-verified", help="Stamp Verified against/Verified at on given rows."
    )
    mark_p.add_argument("provenance_path", help="Path to the claim ledger.")
    mark_p.add_argument(
        "--lines", required=True, help="Comma-separated 1-based row line numbers."
    )
    mark_p.add_argument("--against", required=True, help="Verified-against text.")
    mark_p.add_argument(
        "--at", required=True, type=int, help="Verified-at version number."
    )

    sub.add_parser("new", help="Print an empty ledger skeleton.")

    return p


def main(argv: Optional[List[str]] = None) -> int:
    """CLI entry point. 'check'/'new' always exit ``0`` (advisory).
    'mark-verified' exits ``0`` on success, ``2`` on an invocation
    error."""
    parser = _build_cli_parser()
    args = parser.parse_args(argv)

    try:
        if args.command == "check":
            corpus_roots = [Path(r) for r in args.corpus_roots]
            report = check_claim_ledger(
                Path(args.provenance_path), corpus_roots, args.current_version
            )
            print(json.dumps(report, indent=2))
            return 0
        if args.command == "mark-verified":
            line_nos = [int(x) for x in args.lines.split(",") if x.strip()]
            report = mark_verified(
                Path(args.provenance_path), line_nos, args.against, args.at
            )
            print(json.dumps(report, indent=2))
            return 0
        if args.command == "new":
            print(new_ledger_text(), end="")
            return 0
    except OSError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    return 2  # pragma: no cover - argparse enforces a valid subcommand


__all__ = [
    "KIND_CORPUS",
    "KIND_DERIVED",
    "VERIFICATION_NEVER",
    "VERIFICATION_STALE",
    "VERIFICATION_CURRENT",
    "DEFAULT_HEADER",
    "ClaimRow",
    "ParsedClaimLedger",
    "parse_claim_ledger",
    "unverified_rows",
    "stale_corpus_rows",
    "age_in_versions",
    "check_claim_ledger",
    "mark_verified",
    "summarize",
    "new_ledger_text",
    "main",
]


if __name__ == "__main__":
    raise SystemExit(main())
