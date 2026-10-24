"""Editorial-override matcher for the slides critics (issue #1373).

A document's ``editorial_overrides:`` list (project ``BRIEF.md``) records
operator decisions the critics must not re-flag. Matching is structured
and exact: a finding matches an override only when its ``evidence_span``
names the override's slide (``...:slide=<N>``) AND its ``dimension``
equals the override's dimension. A finding with no slide span or no
dimension never matches, so an override cannot swallow unrelated findings.

Critical flags and ``blocker``-severity findings are never overridden.
"""

from __future__ import annotations

import re
from typing import Any, Dict, Iterable, List, Optional, Tuple

_SLIDE_SPAN_RE = re.compile(r":slide=(\d+)(?!\d)")


def slide_from_span(evidence_span: Optional[str]) -> Optional[int]:
    """Return the slide number from a ``...:slide=<N>`` span, or ``None``."""
    if not evidence_span:
        return None
    m = _SLIDE_SPAN_RE.search(evidence_span)
    return int(m.group(1)) if m else None


def match_override(
    finding: Any, overrides: Iterable[Any]
) -> Optional[Any]:
    """Return the first override matching ``finding`` (slide + dimension), else ``None``.

    ``finding`` needs ``evidence_span`` and ``dimension`` attributes (or
    dict keys); ``overrides`` are ``EditorialOverride`` objects.
    """
    get = (lambda k: finding.get(k)) if isinstance(finding, dict) else (
        lambda k: getattr(finding, k, None)
    )
    slide = slide_from_span(get("evidence_span"))
    dimension = get("dimension")
    if slide is None or dimension is None or not str(dimension).strip():
        return None
    for ov in overrides:
        if ov.slide == slide and ov.dimension == str(dimension).strip():
            return ov
    return None


def overridden_rationale(override: Any, rationale: str) -> str:
    """Prefix ``rationale`` with ``overridden: <reason> (<date>)``."""
    return f"overridden: {override.reason} ({override.date}) — {rationale}"


def apply_overrides(
    findings: List[Dict[str, Any]], overrides: List[Any]
) -> Tuple[List[Dict[str, Any]], List[Any], List[Any]]:
    """Apply overrides to finding dicts (``severity``, ``rationale``, ...).

    Returns ``(findings, applied, unmatched)``. A matched non-critical
    finding is rewritten to ``severity="nit"`` with an ``overridden:``
    rationale prefix (and ``overridden=True``) — it must not be deducted
    from the score. A ``blocker`` finding that matches is left untouched
    and annotated ``override ignored: critical``. ``unmatched`` lists
    overrides that matched nothing (possibly stale after slide renumbering).
    """
    used: List[Any] = []
    out: List[Dict[str, Any]] = []
    for f in findings:
        ov = match_override(f, overrides)
        if ov is None:
            out.append(f)
            continue
        new = dict(f)
        if new.get("severity") == "blocker":
            new["rationale"] = f"override ignored: critical — {new.get('rationale', '')}"
        else:
            new["severity"] = "nit"
            new["rationale"] = overridden_rationale(ov, new.get("rationale", ""))
            new["overridden"] = True
            if ov not in used:
                used.append(ov)
        out.append(new)
    unmatched = [ov for ov in overrides if ov not in used]
    return out, used, unmatched
