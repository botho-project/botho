"""Thread-level ``BRIEF.md`` reader for the proposal skill (issue #356, #840, #1092).

This module ships two load-bearing helpers:

- :func:`load_recommendation_target` — reads the informal-but-now-
  documented ``recommendation_target`` frontmatter key from a proposal
  thread's ``<thread>/BRIEF.md`` and resolves it to a typed signal the
  reviewer can dispatch on at dim 8 (Open decisions) scoring time.
  **Re-exported from** :mod:`anvil.lib.project_brief` (see below) —
  this module no longer defines it locally.
- :func:`load_cost_basis` (issue #840) — reads the ``cost_basis``
  frontmatter key (``quoted`` / ``estimated`` / ``none``) and resolves
  it to a typed signal the drafter, reviewer, and auditor dispatch on
  for the priced-table contract and dim 6 (Cost credibility) scoring.
  Not every proposal is a hardware system with vendor-sourceable BOM
  lines (e.g. a partnership/integration proposal) — ``cost_basis``
  makes that axis explicit and orthogonal to ``customer_kind`` (an
  ``internal`` build spec still answers to a budget with priced lines
  that may or may not be vendor-sourced). This helper stays
  skill-local — it has no consumer outside ``proposal``.

Consolidation history (issue #1092)
------------------------------------

Issue #356 shipped ``load_recommendation_target`` skill-local per
``CLAUDE.md``'s "skill-local first, lib promotion later — wait for the
second consumer before generalizing" (memo's PR #351 was the first
consumer; proposal's #356 was the second, but the calibrated dimension
differs per skill — memo dim 1, proposal dim 8 — so promotion looked
premature at the time).

Commit ``dba8ba1`` (#382, the same day) then promoted *memo's* entire
``project_brief.py`` — including its ``load_recommendation_target`` —
wholesale into ``anvil/lib/project_brief.py`` (memo's skill-local file
became a back-compat shim). That move's scope was rolling the #295/#296
project-org nesting out to deck/slides/proposal, not a deliberate dedup
sweep, so it never touched proposal's already-diverged copy — leaving
proposal silently duplicating a reader function that was, by then,
already centralized.

Issue #1092 closes that gap: the reader function's *body* was verified
byte-identical to the promoted ``anvil/lib/project_brief.py`` version
(the calibrated dimension and rubric prose live in ``rubric.md`` /
``proposal-review.md``, not in the helper, so nothing dimension-specific
was lost). This module now imports the shared implementation instead of
redefining it, mirroring memo's existing shim convention. The
per-dimension calibration prose (dim 8, NOT dim 1 — proposal dim 1 is
*Intent / requirements clarity*, not *Recommendation clarity*) is
unaffected — see ``anvil/skills/proposal/rubric.md`` §"Dim 8 —
`recommendation_target: undecided` calibration".

Lenient contract
----------------

:func:`load_recommendation_target` **never raises**. Every absence /
malformed path resolves to ``None`` — see
:mod:`anvil.lib.project_brief` for the full contract. :func:`load_cost_basis`
below mirrors the same lenient contract verbatim.
"""

from __future__ import annotations

from pathlib import Path
from typing import Literal, Optional

from anvil.lib.frontmatter import extract_frontmatter as _extract_frontmatter
from anvil.lib.project_brief import BRIEF_FILENAME
from anvil.lib.project_brief import load_recommendation_target

# The closed set for `cost_basis` (issue #840). `quoted` is the default and
# is byte-identical to pre-#840 behavior (vendor-sourced priced BOM);
# `estimated` signals unsourced internal engineering estimates; `none`
# signals no hardware BOM / no vendor quotes exist for this proposal at all.
_RECOGNIZED_COST_BASES = ("quoted", "estimated", "none")


__all__ = [
    "BRIEF_FILENAME",
    "load_recommendation_target",
    "load_cost_basis",
]


# ``_extract_frontmatter`` used to be defined here (a local copy mirroring
# memo's); it is now the shared ``anvil/lib/frontmatter.py::extract_frontmatter``
# primitive (issue #1075), imported above and aliased to the historical
# private name so every call site in this module is unchanged.
#
# ``load_recommendation_target`` used to be defined here too (a local copy
# whose body was byte-identical to memo's already-promoted version); it is
# now imported from ``anvil.lib.project_brief`` above (issue #1092). See the
# module docstring's "Consolidation history" section for the full story and
# ``anvil/skills/proposal/rubric.md`` §"Dim 8 — `recommendation_target:
# undecided` calibration" for the proposal-specific calibration this helper
# feeds (unaffected by the consolidation).


def load_cost_basis(
    thread_dir: Path,
) -> Optional[Literal["quoted", "estimated", "none"]]:
    """Read ``cost_basis`` from a thread-level ``BRIEF.md`` (issue #840).

    Not every buildable-system proposal is a hardware system with
    vendor-sourceable BOM lines — a partnership/integration proposal (a
    data-backed challenge to another company, say) has no hardware BOM
    and no vendor quotes at all. Pre-#840, the skill's template, rubric,
    and audit all assumed a priced, vendor-sourced BOM unconditionally
    (the Gossamer LAN worked example's shape), leaving a non-hardware
    proposal with two bad options: manufacture hardware-shaped line
    items to satisfy the template, or omit mandated sections and take a
    structural hit.

    ``cost_basis`` makes the missing axis explicit: **whether priced
    claims have external sources at all** — orthogonal to
    ``customer_kind`` (an ``internal`` build spec still answers to a
    budget with priced lines that may or may not be vendor-sourced).

    Parameters
    ----------
    thread_dir
        The thread root directory (the directory holding ``BRIEF.md``
        for the thread, e.g., ``<project>/<slug>/``). NOT a version
        directory.

    Returns
    -------
    Optional[Literal["quoted", "estimated", "none"]]
        The verbatim ``cost_basis`` value when present and in the
        closed set:

        - ``"quoted"`` — priced lines are (or should be) sourced
          against vendor quotes / datasheets / planning ranges. This is
          the default reading when the key is absent — byte-identical
          to pre-#840 behavior.
        - ``"estimated"`` — priced lines are unsourced internal
          engineering-effort estimates; the template emits estimate-
          labelled captions and dim 6 (Cost credibility) calibrates on
          estimate-basis consistency rather than vendor sourceability.
        - ``"none"`` — there is no hardware BOM and no vendor quotes at
          all; the template drops the priced-table requirement and
          commercial terms move to Open Decisions.

        ``None`` for every absence / malformed path:

        - ``<thread_dir>/BRIEF.md`` does not exist.
        - The file exists but has no YAML frontmatter (no opening
          ``---`` delimiter, missing closing delimiter, malformed
          YAML).
        - The frontmatter is a parseable dict but contains no
          ``cost_basis`` key.
        - The frontmatter value is not in the closed set (``quoted`` /
          ``estimated`` / ``none``) — e.g., ``Quoted`` (capitalized),
          ``vendor``, an integer, a list, a null. Callers fall back to
          byte-identical pre-#840 behavior (the same as ``"quoted"``)
          for these noise values.

    Notes
    -----
    Lenient by design — never raises. The contract mirrors
    :func:`load_recommendation_target` exactly: every absence /
    malformed path resolves to ``None``, preserving byte-identical
    pre-#840 behavior for any thread that does not declare
    ``cost_basis``.
    """
    if not isinstance(thread_dir, Path):
        # Defensive: callers may inadvertently pass a string. The helper is
        # documented to take a Path; convert rather than raise to preserve
        # the lenient contract.
        try:
            thread_dir = Path(thread_dir)
        except Exception:
            return None

    brief_path = thread_dir / BRIEF_FILENAME
    if not brief_path.is_file():
        return None

    try:
        text = brief_path.read_text(encoding="utf-8")
    except OSError:
        return None

    fm = _extract_frontmatter(text)
    if fm is None:
        return None

    value = fm.get("cost_basis")
    # Closed-set membership check. Anything not on the recognized list —
    # including booleans, ints, lists, dicts, None, and string typos —
    # falls through to None per the lenient contract.
    if isinstance(value, str) and value in _RECOGNIZED_COST_BASES:
        return value  # type: ignore[return-value]
    return None
