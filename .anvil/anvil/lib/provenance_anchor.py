"""Deterministic ``provenance.md`` anchor-drift advisory (issue #868).

The problem
-----------

The local-corpus claim-provenance contract (issue #597,
``anvil/lib/snippets/provenance.md``) has each ``provenance.md`` row cite
its supporting corpus passage by a bare ``Source file`` + ``Line range``.
That address is not stable: any mid-file edit of the corpus file (an
insertion above the cited passage, a reflow, an appended correction) can
silently shift every row that cites a later line — the citation still
*resolves* to a range, just the wrong text. A spot-sampling reviewer
reading plausible text at the (now-wrong) cited range passes it; only an
exhaustive corpus audit that re-opens every range catches the drift, and
only if it re-runs after the corpus changed.

This module implements the fix's mechanical half: each provenance row MAY
carry an **``Anchor``** column — a short verbatim quoted snippet drawn
from the cited passage — which is the row's true, content-addressed
identity. The ``Line range`` column is demoted to a *hint*: cheap to
read, but not authoritative. This module:

1. **Resolves** each row's anchor against the on-disk corpus file,
   searching the WHOLE file (not just the hinted range) for the anchor
   text and classifying the outcome (:data:`STATUS_RESOLVED` /
   :data:`STATUS_DRIFTED` / :data:`STATUS_NOT_FOUND` /
   :data:`STATUS_FILE_NOT_FOUND` / :data:`STATUS_NO_ANCHOR`).
2. **Repoints** drifted rows mechanically — rewriting only the ``Line
   range`` cell to the anchor's actual current location, leaving
   ``Claim`` / ``Source file`` / ``Anchor`` / ``Notes`` untouched. This is
   explicitly NOT the "fabricating a source-line mapping" failure the
   drafter/reviser contract prohibits: the anchor text itself already
   proves the citation is genuine, this only corrects a stale hint.

Distinct from :mod:`anvil.lib.evidence_drift` (issue #857, BRIEF/refs
mtime staleness) and :mod:`anvil.lib.probe_freshness` (issue #863,
perishable external claims). Both of those detect that a *verification*
has gone stale because something *outside* the citation moved. This
module detects that the citation's own *address* has gone stale because
the *cited file* moved underneath it — the evidence never changed, only
where it lives. See ``anvil/lib/snippets/provenance.md`` §"Relationship
to #863" for the boundary discussion.

Backward compatibility
-----------------------

A ``provenance.md`` row written before this feature has no ``Anchor``
column value (either the whole table predates the 5-column shape, or an
individual row's ``Anchor`` cell is empty). Both are reported as
:data:`STATUS_NO_ANCHOR` — never an error, never a false drift signal.
Drift detection is simply unavailable for that row until the next
draft/revise pass adds an anchor. This mirrors
:mod:`anvil.lib.evidence_drift`'s ``STATUS_NO_SNAPSHOT`` bootstrap
posture: unknown is honest, never conflated with "not drifted" OR with
"drifted".

Matching is exact-after-normalization (curly quotes folded to straight,
whitespace runs collapsed to a single space), case-sensitive — the same
verbatim-quote discipline as ``anvil/lib/evidence_check.py``, applied to
corpus passages instead of a reviewed body.

CLI entry-point
----------------

``python -m anvil.lib.provenance_anchor check <provenance.md> <corpus_root> [<corpus_root> ...]``
    Resolves every row's anchor against the given corpus roots and prints
    the report as JSON. Exit code is always ``0`` (invocation errors
    aside) — this tool is advisory; the calling critic decides how to
    weigh a ``DRIFTED``/``NOT_FOUND`` row.

``python -m anvil.lib.provenance_anchor repoint <provenance.md> <corpus_root> [<corpus_root> ...]``
    Mechanically rewrites the ``Line range`` cell of every
    :data:`STATUS_DRIFTED` row to the anchor's resolved current location.
    Leaves every other cell, and every non-drifted row, byte-identical.
    Prints a summary of what was repointed as JSON. Atomic via
    tmp-then-``os.replace`` (the same primitive as
    ``anvil/lib/cite.py::_cache_write`` /
    ``anvil/lib/evidence_drift.py::_atomic_write_json``).
"""

from __future__ import annotations

import json
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

# ---------------------------------------------------------------------------
# Status vocabulary
# ---------------------------------------------------------------------------

STATUS_NO_ANCHOR = "NO_ANCHOR"
"""The row has no ``Anchor`` cell value (legacy row predating this
feature, or an unmigrated row) — drift detection unavailable. Never a
drift signal, never an error."""

STATUS_FILE_NOT_FOUND = "FILE_NOT_FOUND"
"""``Source file`` does not resolve under any given corpus root."""

STATUS_NOT_FOUND = "NOT_FOUND"
"""The anchor text is not present anywhere in the resolved source file —
the passage was deleted or rewritten, not merely moved. Degrades to the
five-way vocabulary's ``NOT_FOUND`` classification, NOT a drift finding
(edge case (a) from issue #868's test plan)."""

STATUS_RESOLVED = "RESOLVED"
"""The anchor text is present and its location overlaps the cited
``Line range`` hint (or the row has no parseable hint to compare
against) — no drift."""

STATUS_DRIFTED = "DRIFTED"
"""The anchor text is present verbatim in the source file but at a
location that does NOT overlap the cited ``Line range`` hint — the hint
is stale, the citation itself is not. Distinct from
:data:`STATUS_NOT_FOUND` / a content mismatch classification."""

_STATUSES = (
    STATUS_NO_ANCHOR,
    STATUS_FILE_NOT_FOUND,
    STATUS_NOT_FOUND,
    STATUS_RESOLVED,
    STATUS_DRIFTED,
)

_CURLY_FOLD = {
    "“": '"',
    "”": '"',
    "‘": "'",
    "’": "'",
}


def normalize(text: str) -> str:
    """Fold curly quotes to straight and collapse whitespace runs.

    Case-sensitive by contract — an anchor is a verbatim quote or it is
    not evidence, the same posture as ``evidence_check.py::normalize``.
    """
    for curly, straight in _CURLY_FOLD.items():
        text = text.replace(curly, straight)
    return " ".join(text.split())


# ---------------------------------------------------------------------------
# provenance.md table parsing
# ---------------------------------------------------------------------------

_LINE_RANGE_RE = re.compile(r"(\d+)\s*(?:-\s*(\d+))?")


def _parse_line_range(cell: str) -> Optional[Tuple[int, int]]:
    """Parse a ``Line range`` cell (``"412-415"``, ``"7"``, ``"L412-415"``).

    Returns ``None`` for anything unparsable (``"?"``, ``"NOT_FOUND"``,
    empty) — a row with no usable hint, not an error.
    """
    m = _LINE_RANGE_RE.search(cell or "")
    if not m:
        return None
    start = int(m.group(1))
    end = int(m.group(2)) if m.group(2) else start
    if end < start:
        start, end = end, start
    return (start, end)


def _split_row(line: str) -> List[str]:
    """Split a ``| a | b | c |`` markdown table row into stripped cells."""
    stripped = line.strip()
    if stripped.startswith("|"):
        stripped = stripped[1:]
    if stripped.endswith("|"):
        stripped = stripped[:-1]
    return [cell.strip() for cell in stripped.split("|")]


def _is_separator_row(cells: Sequence[str]) -> bool:
    return bool(cells) and all(re.fullmatch(r":?-{2,}:?", c.strip()) for c in cells)


@dataclass
class ProvenanceRow:
    """One parsed ``provenance.md`` table row."""

    line_no: int
    """1-based line number of this row within ``provenance.md`` (for
    ``evidence_span``-style back-references, e.g. ``provenance.md:L<N>``)."""

    claim: str
    source_file: str
    line_range_raw: str
    line_range: Optional[Tuple[int, int]]
    anchor: Optional[str]
    """``None`` when the row has no ``Anchor`` column value at all — a
    legacy (pre-#868) row or unmigrated table shape."""

    notes: str


@dataclass
class ParsedProvenanceTable:
    """A parsed ``provenance.md`` file: header + rows + the raw lines
    (kept for :func:`repoint_drifted_anchors` to rewrite in place)."""

    header: List[str]
    line_range_col: Optional[int]
    anchor_col: Optional[int]
    rows: List[ProvenanceRow]
    lines: List[str]


def parse_provenance_table(path: Path) -> ParsedProvenanceTable:
    """Parse a ``provenance.md`` claim->source map.

    Tolerates both the legacy 4-column shape (``Claim | Source file |
    Line range | Notes``) and the anchor-bearing 5-column shape
    (``Claim | Source file | Line range | Anchor | Notes``) — column
    identity is derived from the header row, not a fixed position, so
    either shape (or a consumer-reordered variant) parses correctly.
    Rows in a legacy table (no ``Anchor`` header) get ``anchor=None``
    (:data:`STATUS_NO_ANCHOR`) rather than raising.
    """
    lines = Path(path).read_text(encoding="utf-8").splitlines()

    header: List[str] = []
    header_idx: Optional[int] = None
    for idx, line in enumerate(lines):
        if not line.strip().startswith("|"):
            continue
        cells = _split_row(line)
        lowered = [c.lower() for c in cells]
        if "claim" in lowered and any("source" in c for c in lowered):
            header = cells
            header_idx = idx
            break

    if header_idx is None:
        return ParsedProvenanceTable(
            header=[], line_range_col=None, anchor_col=None, rows=[], lines=lines
        )

    lowered_header = [c.lower() for c in header]

    def _find_col(*names: str) -> Optional[int]:
        for i, cell in enumerate(lowered_header):
            if any(name in cell for name in names):
                return i
        return None

    claim_col = _find_col("claim")
    source_col = _find_col("source")
    line_range_col = _find_col("line range", "line-range")
    anchor_col = _find_col("anchor")
    notes_col = _find_col("notes")

    rows: List[ProvenanceRow] = []
    for row_idx in range(header_idx + 1, len(lines)):
        line = lines[row_idx]
        if not line.strip().startswith("|"):
            break
        cells = _split_row(line)
        if _is_separator_row(cells):
            continue
        if len(cells) < 2:
            continue

        def _cell(col: Optional[int]) -> str:
            if col is None or col >= len(cells):
                return ""
            return cells[col]

        claim = _cell(claim_col)
        source_file = _cell(source_col)
        line_range_raw = _cell(line_range_col)
        notes = _cell(notes_col)
        anchor_cell = _cell(anchor_col) if anchor_col is not None else None
        anchor = anchor_cell.strip("\"'“” ") if anchor_cell else None
        if anchor is not None and not anchor:
            anchor = None

        rows.append(
            ProvenanceRow(
                line_no=row_idx + 1,
                claim=claim,
                source_file=source_file,
                line_range_raw=line_range_raw,
                line_range=_parse_line_range(line_range_raw),
                anchor=anchor,
                notes=notes,
            )
        )

    return ParsedProvenanceTable(
        header=header,
        line_range_col=line_range_col,
        anchor_col=anchor_col,
        rows=rows,
        lines=lines,
    )


# ---------------------------------------------------------------------------
# Corpus file resolution
# ---------------------------------------------------------------------------


def resolve_source_file(
    source_file: str, corpus_roots: Sequence[Path]
) -> Optional[Path]:
    """Resolve ``source_file`` (a path relative to a declared corpus dir)
    against the given ``corpus_roots``, in order. ``None`` if it
    resolves under none of them (:data:`STATUS_FILE_NOT_FOUND`)."""
    for root in corpus_roots:
        candidate = Path(root) / source_file
        if candidate.is_file():
            return candidate
    return None


# ---------------------------------------------------------------------------
# Anchor resolution
# ---------------------------------------------------------------------------


def _normalize_with_line_map(text: str) -> Tuple[str, List[int]]:
    """Normalize ``text`` the way :func:`normalize` does, but also return
    a parallel list mapping each output character's index back to its
    1-based source line number — needed to translate a match position
    back into a citable line range."""
    out_chars: List[str] = []
    out_lines: List[int] = []
    lineno = 1
    pending_space = False
    started = False
    for ch in text:
        if ch == "\n":
            lineno += 1
            pending_space = True
            continue
        folded = _CURLY_FOLD.get(ch, ch)
        if folded.isspace():
            pending_space = True
            continue
        if pending_space and started:
            out_chars.append(" ")
            out_lines.append(lineno)
        pending_space = False
        started = True
        out_chars.append(folded)
        out_lines.append(lineno)
    return "".join(out_chars), out_lines


def _find_all(haystack: str, needle: str) -> List[int]:
    if not needle:
        return []
    idxs: List[int] = []
    start = 0
    while True:
        idx = haystack.find(needle, start)
        if idx == -1:
            break
        idxs.append(idx)
        start = idx + 1
    return idxs


def _ranges_overlap(a: Tuple[int, int], b: Tuple[int, int]) -> bool:
    return a[0] <= b[1] and b[0] <= a[1]


@dataclass
class AnchorResolution:
    """Outcome of resolving one :class:`ProvenanceRow`'s anchor."""

    row: ProvenanceRow
    status: str
    resolved_range: Optional[Tuple[int, int]]
    occurrences: int
    detail: str

    @property
    def drifted(self) -> bool:
        return self.status == STATUS_DRIFTED

    def to_json(self) -> Dict[str, Any]:
        return {
            "row_line": self.row.line_no,
            "claim": self.row.claim,
            "source_file": self.row.source_file,
            "line_range_hint": list(self.row.line_range)
            if self.row.line_range
            else None,
            "anchor": self.row.anchor,
            "status": self.status,
            "resolved_range": list(self.resolved_range)
            if self.resolved_range
            else None,
            "occurrences": self.occurrences,
            "detail": self.detail,
        }


def resolve_anchor(
    file_path: Optional[Path], row: ProvenanceRow
) -> AnchorResolution:
    """Resolve one row's anchor against ``file_path`` (already resolved
    via :func:`resolve_source_file`; ``None`` means unresolvable)."""
    if not row.anchor:
        return AnchorResolution(
            row=row,
            status=STATUS_NO_ANCHOR,
            resolved_range=None,
            occurrences=0,
            detail=(
                "row has no Anchor column value (legacy row predating "
                "issue #868, or not yet migrated) — anchor-drift "
                "detection is unavailable for this row until the next "
                "draft/revise pass adds one."
            ),
        )

    if file_path is None:
        return AnchorResolution(
            row=row,
            status=STATUS_FILE_NOT_FOUND,
            resolved_range=None,
            occurrences=0,
            detail=(
                f"source file '{row.source_file}' is not resolvable "
                "under any declared corpus root."
            ),
        )

    normalized_anchor = normalize(row.anchor)
    if not normalized_anchor:
        return AnchorResolution(
            row=row,
            status=STATUS_NO_ANCHOR,
            resolved_range=None,
            occurrences=0,
            detail="row's Anchor cell is empty after normalization.",
        )

    text = file_path.read_text(encoding="utf-8", errors="replace")
    normalized_text, line_map = _normalize_with_line_map(text)
    idxs = _find_all(normalized_text, normalized_anchor)

    if not idxs:
        return AnchorResolution(
            row=row,
            status=STATUS_NOT_FOUND,
            resolved_range=None,
            occurrences=0,
            detail=(
                f"anchor text not found anywhere in '{row.source_file}' "
                "— the passage was deleted or rewritten, not merely "
                "moved (degrades to the five-way NOT_FOUND "
                "classification, not a drift finding)."
            ),
        )

    candidates: List[Tuple[int, int]] = []
    for idx in idxs:
        start_line = line_map[idx]
        end_idx = min(idx + len(normalized_anchor) - 1, len(line_map) - 1)
        end_line = line_map[end_idx]
        candidates.append((start_line, end_line))

    hint = row.line_range
    if hint is not None:
        chosen = min(candidates, key=lambda c: abs(c[0] - hint[0]))
    else:
        chosen = candidates[0]

    ambiguous_note = ""
    if len(candidates) > 1:
        ambiguous_note = (
            f" ({len(candidates)} occurrences found in the file; resolved "
            "to the one nearest the cited Line range hint.)"
        )

    if hint is None:
        return AnchorResolution(
            row=row,
            status=STATUS_RESOLVED,
            resolved_range=chosen,
            occurrences=len(candidates),
            detail=(
                f"anchor resolved to '{row.source_file}':{chosen[0]}-"
                f"{chosen[1]}; row has no parseable Line range hint to "
                "compare against." + ambiguous_note
            ),
        )

    if _ranges_overlap(chosen, hint):
        return AnchorResolution(
            row=row,
            status=STATUS_RESOLVED,
            resolved_range=chosen,
            occurrences=len(candidates),
            detail=(
                f"anchor found at '{row.source_file}':{chosen[0]}-"
                f"{chosen[1]}, matching the cited hint "
                f"{hint[0]}-{hint[1]}." + ambiguous_note
            ),
        )

    return AnchorResolution(
        row=row,
        status=STATUS_DRIFTED,
        resolved_range=chosen,
        occurrences=len(candidates),
        detail=(
            f"anchor text is still present verbatim in "
            f"'{row.source_file}' but now at line {chosen[0]}-{chosen[1]} "
            f"(row cites {hint[0]}-{hint[1]}) — the Line range hint is "
            "stale, not the claim. Distinct from a content MISMATCH: the "
            "cited passage was found, just not where the row says it "
            "is." + ambiguous_note
        ),
    )


# ---------------------------------------------------------------------------
# Whole-table check
# ---------------------------------------------------------------------------


def check_provenance_anchors(
    provenance_path: Path, corpus_roots: Sequence[Path]
) -> Dict[str, Any]:
    """Resolve every row of ``provenance_path`` against ``corpus_roots``.

    Returns a JSON-serializable report: per-row resolutions plus summary
    counts across the five statuses. Never raises for a missing/empty
    provenance file — an empty table reports zero rows.
    """
    table = parse_provenance_table(provenance_path)
    counts: Dict[str, int] = {status: 0 for status in _STATUSES}
    resolutions: List[AnchorResolution] = []

    for row in table.rows:
        file_path = resolve_source_file(row.source_file, corpus_roots)
        resolution = resolve_anchor(file_path, row)
        resolutions.append(resolution)
        counts[resolution.status] += 1

    return {
        "provenance_path": str(provenance_path),
        "total_rows": len(table.rows),
        "anchor_column_present": table.anchor_col is not None,
        "counts": counts,
        "drifted": counts[STATUS_DRIFTED] > 0,
        "rows": [r.to_json() for r in resolutions],
    }


# ---------------------------------------------------------------------------
# Mechanical repoint
# ---------------------------------------------------------------------------


def _format_line_range(rng: Tuple[int, int]) -> str:
    start, end = rng
    return str(start) if start == end else f"{start}-{end}"


def _rewrite_line_range_cell(line: str, col_index: int, new_value: str) -> str:
    """Rewrite the ``col_index``-th cell of a markdown table row to
    ``new_value``, preserving every other cell and the row's leading/
    trailing pipe style."""
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


def repoint_drifted_anchors(
    provenance_path: Path, corpus_roots: Sequence[Path]
) -> Dict[str, Any]:
    """Mechanically rewrite the ``Line range`` cell of every
    :data:`STATUS_DRIFTED` row to its anchor's resolved current location.

    Only :data:`STATUS_DRIFTED` rows are touched; every other row (and
    every non-table line — prose, headings) is written back byte-
    identical. A no-op (returns ``repointed: []``) when the table has no
    ``Anchor`` column, no rows, or nothing drifted. Atomic write via
    tmp-then-``os.replace``.
    """
    table = parse_provenance_table(provenance_path)
    repointed: List[Dict[str, Any]] = []

    if table.line_range_col is None or table.anchor_col is None:
        return {
            "provenance_path": str(provenance_path),
            "repointed": repointed,
            "detail": (
                "table has no Line range and/or Anchor column — nothing "
                "to mechanically repoint."
            ),
        }

    new_lines = list(table.lines)
    for row in table.rows:
        file_path = resolve_source_file(row.source_file, corpus_roots)
        resolution = resolve_anchor(file_path, row)
        if resolution.status != STATUS_DRIFTED or resolution.resolved_range is None:
            continue
        idx = row.line_no - 1
        new_value = _format_line_range(resolution.resolved_range)
        new_lines[idx] = _rewrite_line_range_cell(
            new_lines[idx], table.line_range_col, new_value
        )
        repointed.append(
            {
                "row_line": row.line_no,
                "claim": row.claim,
                "source_file": row.source_file,
                "old_line_range": row.line_range_raw,
                "new_line_range": new_value,
            }
        )

    if repointed:
        _atomic_write_text(
            Path(provenance_path), "\n".join(new_lines) + "\n"
        )

    return {
        "provenance_path": str(provenance_path),
        "repointed": repointed,
        "detail": (
            f"mechanically repointed {len(repointed)} drifted row(s); "
            "Claim/Source file/Anchor/Notes left untouched."
            if repointed
            else "no drifted rows found — nothing repointed."
        ),
    }


def _atomic_write_text(path: Path, content: str) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(content, encoding="utf-8")
    os.replace(tmp, path)


# ---------------------------------------------------------------------------
# CLI entry-point
# ---------------------------------------------------------------------------


def _build_cli_parser():
    import argparse

    p = argparse.ArgumentParser(
        prog="python -m anvil.lib.provenance_anchor",
        description=(
            "Deterministic provenance.md anchor-drift advisory (issue "
            "#868). 'check' reports drift; 'repoint' mechanically fixes "
            "the Line range hint of drifted rows."
        ),
    )
    sub = p.add_subparsers(dest="command", required=True)

    check_p = sub.add_parser(
        "check",
        help="Resolve every row's anchor and report drift status as JSON.",
    )
    check_p.add_argument("provenance_path", help="Path to provenance.md.")
    check_p.add_argument(
        "corpus_roots", nargs="+", help="One or more resolved corpus root dirs."
    )

    repoint_p = sub.add_parser(
        "repoint",
        help="Mechanically rewrite drifted rows' Line range cell in place.",
    )
    repoint_p.add_argument("provenance_path", help="Path to provenance.md.")
    repoint_p.add_argument(
        "corpus_roots", nargs="+", help="One or more resolved corpus root dirs."
    )

    return p


def main(argv: Optional[List[str]] = None) -> int:
    """CLI entry point. Always exits ``0`` on a successful invocation of
    either subcommand (both are advisory/mechanical, never a pass/fail
    gate); ``2`` on an invocation error."""
    parser = _build_cli_parser()
    args = parser.parse_args(argv)

    try:
        corpus_roots = [Path(r) for r in args.corpus_roots]
        if args.command == "check":
            report = check_provenance_anchors(Path(args.provenance_path), corpus_roots)
            print(json.dumps(report, indent=2))
            return 0
        if args.command == "repoint":
            report = repoint_drifted_anchors(Path(args.provenance_path), corpus_roots)
            print(json.dumps(report, indent=2))
            return 0
    except OSError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    return 2  # pragma: no cover - argparse enforces a valid subcommand


__all__ = [
    "STATUS_NO_ANCHOR",
    "STATUS_FILE_NOT_FOUND",
    "STATUS_NOT_FOUND",
    "STATUS_RESOLVED",
    "STATUS_DRIFTED",
    "normalize",
    "ProvenanceRow",
    "ParsedProvenanceTable",
    "parse_provenance_table",
    "resolve_source_file",
    "AnchorResolution",
    "resolve_anchor",
    "check_provenance_anchors",
    "repoint_drifted_anchors",
    "main",
]


if __name__ == "__main__":
    raise SystemExit(main())
