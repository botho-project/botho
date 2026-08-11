"""Deterministic cross-version drift metrics for the ``anvil:memo`` skill (issue #746).

No phase in the memo lifecycle can see a cross-version trend. `memo-review`
deliberately scores each version "on its own rubric merits" — it does not
read prior-pass metadata, and doctrine holds that this is correct *for
scores* (it prevents anchoring). But the same doctrine is wrong *for style
trends*: a document that gets a few percentage points denser every revision
cycle looks, at every single review, like a static style choice. The canary
evidence (studio, sentinel thread, 2026-07-27) is a 4-version thread whose
bold-word share ratcheted 19.9% -> 20.7% -> 25.1% -> 25.5% — monotonic
across every transition, and no single review (each scored the version in
front of it) ever flagged the trend.

This module is the missing sensor: given the CURRENT version dir of a
thread, it resolves the immediately-prior version (``N-1``) and, when
available, the version before that (``N-2``), computes seven mechanical
metrics for each, and — for the three **densification metrics** (bold-word
share, hedge-marker count, meta-commentary count) — emits one finding when
a metric increased across BOTH of the last two version transitions. A
single-transition increase is observational only (a legitimate revision
can add emphasis once); the rule exists to catch the *ratchet*, not any
one increase.

This is purely mechanical — no judgment, no LLM variance, no critical-flag
participation. ``memo-review`` step 4m invokes :func:`compute_drift` and
folds any findings into ``comments.md`` at ``major`` severity / ``scope:
reduce`` (see ``rubric.md`` §"Version drift" and ``commands/memo-review.md``
step 4m / step 8), so the default ``--scope important`` reviser at
`memo-revise` MUST address them — and the natural resolution (cut the
ratcheted emphasis/hedging/meta-commentary back toward the earlier
version's level) is subtractive by construction.

Metrics
-------

- ``word_count`` — total words in the body (code-fence and inline-code
  spans excluded from the scan).
- ``bold_span_count`` — number of ``**bold**`` markdown spans.
- ``bold_word_count`` / ``bold_word_share`` — words wrapped in bold spans,
  and that count as a percentage of ``word_count``.
- ``hedge_marker_count`` — hits against a fixed, small lexicon
  (``reportedly``, ``roughly``, ``founder-reported``, ``unreplicated``)
  plus parenthetical spans that read as a caveat (``(unconfirmed)``,
  ``(self-reported)``, ``(pending confirmation)``, etc.).
- ``meta_commentary_count`` — hits against a fixed, small lexicon of
  self-referential phrases (``as noted above``, ``as previously
  mentioned``, ``to reiterate``, ...). This is an **interim default**
  pending the canonical meta-commentary rule set anvil#745 is adding to
  ``anvil.lib.rhetoric_lint``; once that ships, a follow-on MAY swap this
  module's lexicon to delegate to the shared lint instead of maintaining
  a second one. Shipping the interim lexicon now (rather than blocking on
  #745) is a deliberate choice — the sensor gap is the load-bearing
  problem the issue names, and the exact meta-commentary vocabulary is
  refinable in place without a schema change.
- ``mean_sentence_length`` — mean words per sentence (headings, blank
  lines, code fences, and inline code are excluded from the scan).
- ``exhibit_count`` — number of files under ``exhibits/`` in the version
  dir.

Finding rule
------------

Only the three **densification metrics** (``bold_word_share``,
``hedge_marker_count``, ``meta_commentary_count``) participate in the
finding rule. ``word_count``, ``bold_span_count``, ``mean_sentence_length``,
and ``exhibit_count`` are reported in the raw metrics/transitions block for
operator context but never emit a finding on their own — they are not
densification signals in the same sense (a longer memo is not necessarily
a denser one).

A finding fires when a densification metric increased in **both** of the
last two version transitions (``N-2 -> N-1`` AND ``N-1 -> N``). This
requires three versions to be present and readable; when only ``N`` and
``N-1`` are available (thread with exactly two versions, or ``N-2``'s body
file is missing), the check still runs and reports the single transition,
but emits no findings — a single-transition increase is observational
only.

Graceful-skip
-------------

``N == 1`` (first version of the thread) or a missing/unreadable
``<thread>.{N-1}/`` body returns ``VersionDriftResult(ran=False,
reason=...)`` with zero findings. This is the steady-state default before
this contract shipped, and it is the permanent contract for any thread's
first iteration.

Public API
----------

- :func:`compute_metrics` — mechanical metrics for one version dir.
- :func:`compute_drift` — resolves ``N-1`` / ``N-2`` from a version dir's
  own name (the ``<thread>.<N>`` naming convention) and runs the full
  drift check.
- :class:`VersionMetrics` / :class:`DriftFinding` / :class:`VersionDriftResult`
  — JSON-serializable via ``to_dict()``.

Pure stdlib (``re``, ``dataclasses``, ``pathlib``) — no third-party
imports, matching the optional-extras philosophy in ``pyproject.toml``.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

#: Version-dir naming convention: ``<thread-slug>.<N>``. No zero-pad, no
#: nested dots in the numeric suffix (per
#: ``anvil/lib/snippets/version_layout.md``).
_VERSION_DIR_RE = re.compile(r"^(?P<slug>.+)\.(?P<n>\d+)$")

#: Word tokens for word/sentence counting. Hyphenated and apostrophized
#: compounds count once ("fast-paced", "it's") — mirrors the tokenizer in
#: ``anvil/lib/rhetoric_lint.py``.
_WORD_RE = re.compile(r"[A-Za-z0-9]+(?:['’\-][A-Za-z0-9]+)*")

#: Code-fence opener/closer (``` or ~~~).
_FENCE_RE = re.compile(r"^ {0,3}(```|~~~)")

#: Inline code span: `...` (single line, non-greedy).
_INLINE_CODE_RE = re.compile(r"`[^`\n]*`")

#: Markdown bold span. Restricted to a single line (no embedded newline,
#: no embedded literal ``**``) — matches the overwhelming majority of
#: real-world usage and avoids pathological cross-paragraph matches.
_BOLD_RE = re.compile(r"\*\*([^\*\n]+?)\*\*")

#: ATX heading line.
_HEADING_RE = re.compile(r"^#{1,6}\s")

#: Sentence boundary: end punctuation followed by whitespace.
_SENTENCE_SPLIT_RE = re.compile(r"(?<=[.!?])\s+")

#: Fixed hedge-marker lexicon (issue #746 acceptance sketch).
HEDGE_PHRASES: tuple[str, ...] = (
    "reportedly",
    "roughly",
    "founder-reported",
    "unreplicated",
)
_HEDGE_PHRASE_RE = re.compile(
    r"\b(?:" + "|".join(re.escape(p) for p in HEDGE_PHRASES) + r")\b",
    re.IGNORECASE,
)

#: Non-nested parenthetical span.
_PAREN_RE = re.compile(r"\(([^()]*)\)")

#: Caveat markers that, inside a parenthetical, count as a hedge instance
#: distinct from the literal :data:`HEDGE_PHRASES` (kept non-overlapping
#: to avoid double-counting a phrase like "(roughly 40%)").
CAVEAT_PAREN_MARKERS: tuple[str, ...] = (
    "unconfirmed",
    "unverified",
    "self-reported",
    "not independently verified",
    "pending confirmation",
    "tbd",
    "to be confirmed",
    "per founder",
    "per management",
    "no independent verification",
)
_CAVEAT_PAREN_RE = re.compile(
    "|".join(re.escape(m) for m in CAVEAT_PAREN_MARKERS), re.IGNORECASE
)

#: Interim meta-commentary lexicon — see module docstring §"Meta-commentary
#: count" for the anvil#745 relationship.
META_COMMENTARY_PHRASES: tuple[str, ...] = (
    "as noted above",
    "as noted below",
    "as mentioned above",
    "as mentioned earlier",
    "as discussed above",
    "as discussed below",
    "as previously mentioned",
    "as previously noted",
    "as we noted",
    "as we discussed",
    "as covered earlier",
    "to reiterate",
    "worth reiterating",
    "as stated earlier",
    "circling back to",
    "returning to the earlier point",
)
_META_COMMENTARY_RE = re.compile(
    "|".join(re.escape(p) for p in META_COMMENTARY_PHRASES), re.IGNORECASE
)

#: The three densification metrics that participate in the finding rule.
#: Order is documentation-stable (matches the issue body's listing).
DENSIFICATION_METRICS: tuple[str, ...] = (
    "bold_word_share",
    "hedge_marker_count",
    "meta_commentary_count",
)

#: All metrics reported in the raw metrics/transitions block.
ALL_METRICS: tuple[str, ...] = (
    "word_count",
    "bold_span_count",
    "bold_word_share",
    "hedge_marker_count",
    "meta_commentary_count",
    "mean_sentence_length",
    "exhibit_count",
)

_METRIC_LABELS: dict[str, str] = {
    "word_count": "word count",
    "bold_span_count": "bold span count",
    "bold_word_share": "bold-word share",
    "hedge_marker_count": "hedge-marker count",
    "meta_commentary_count": "meta-commentary count",
    "mean_sentence_length": "mean sentence length",
    "exhibit_count": "exhibit count",
}

#: Fixed severity/scope for every finding this module emits — see module
#: docstring. Never varies; not derived per-instance.
FINDING_SEVERITY = "major"
FINDING_SCOPE = "reduce"


# ---------------------------------------------------------------------------
# Scan preprocessing
# ---------------------------------------------------------------------------


def _strip_code(text: str) -> str:
    """Blank fenced code blocks and inline code spans; preserve line count."""
    out: list[str] = []
    in_fence = False
    fence_marker = ""
    for raw in text.splitlines():
        stripped = raw.lstrip()
        m = _FENCE_RE.match(raw)
        if m:
            if not in_fence:
                in_fence = True
                fence_marker = m.group(1)
            elif stripped.startswith(fence_marker):
                in_fence = False
            out.append("")
            continue
        if in_fence:
            out.append("")
            continue
        out.append(_INLINE_CODE_RE.sub(" ", raw))
    return "\n".join(out)


def _prose_only(text: str) -> str:
    """Code-stripped text with heading lines and blank lines removed.

    Used for sentence-length computation, where headings would otherwise
    masquerade as (typically short, punctuation-free) sentences.
    """
    lines = []
    for line in _strip_code(text).splitlines():
        if _HEADING_RE.match(line):
            continue
        if not line.strip():
            continue
        lines.append(line)
    return " ".join(lines)


# ---------------------------------------------------------------------------
# Result types
# ---------------------------------------------------------------------------


@dataclass
class VersionMetrics:
    """The seven mechanical metrics for one version dir."""

    version: int
    word_count: int
    bold_span_count: int
    bold_word_count: int
    bold_word_share: float  # percentage, 0-100
    hedge_marker_count: int
    meta_commentary_count: int
    mean_sentence_length: float
    exhibit_count: int

    def to_dict(self) -> dict:
        return {
            "version": self.version,
            "word_count": self.word_count,
            "bold_span_count": self.bold_span_count,
            "bold_word_count": self.bold_word_count,
            "bold_word_share": round(self.bold_word_share, 2),
            "hedge_marker_count": self.hedge_marker_count,
            "meta_commentary_count": self.meta_commentary_count,
            "mean_sentence_length": round(self.mean_sentence_length, 2),
            "exhibit_count": self.exhibit_count,
        }


@dataclass
class DriftFinding:
    """One two-transition monotonic-densification finding.

    Fixed ``severity="major"`` / ``scope="reduce"`` per the module
    contract — every finding this module emits is the same class of
    thing (a ratchet the reviewer should surface as a `comments.md`
    entry the default ``--scope important`` reviser must address).
    """

    metric: str
    metric_label: str
    versions: list[int]  # [N-2, N-1, N]
    values: list[float]
    trajectory: str  # formatted "19.9% -> 20.7% -> 25.1%"
    message: str
    suggested_fix: str
    severity: str = FINDING_SEVERITY
    scope: str = FINDING_SCOPE

    def to_dict(self) -> dict:
        return {
            "metric": self.metric,
            "metric_label": self.metric_label,
            "versions": list(self.versions),
            "values": list(self.values),
            "trajectory": self.trajectory,
            "severity": self.severity,
            "scope": self.scope,
            "message": self.message,
            "suggested_fix": self.suggested_fix,
        }


@dataclass
class VersionDriftResult:
    """Outcome of one version-drift check.

    ``ran=False`` covers every graceful-skip path (first version, no
    prior version dir, missing/unreadable body). ``ran=True`` always
    carries ``current_version`` / ``prior_version`` / ``metrics`` /
    ``transitions``; ``earliest_version`` and the ``earliest_to_prior``
    transition are present only when ``<thread>.{N-2}/`` was also
    resolvable.
    """

    ran: bool
    reason: Optional[str] = None
    current_version: Optional[int] = None
    prior_version: Optional[int] = None
    earliest_version: Optional[int] = None
    metrics: dict[int, VersionMetrics] = field(default_factory=dict)
    transitions: dict[str, dict] = field(default_factory=dict)
    findings: list[DriftFinding] = field(default_factory=list)

    @property
    def findings_count(self) -> int:
        return len(self.findings)

    def to_dict(self) -> dict:
        if not self.ran:
            return {"ran": False, "reason": self.reason}
        return {
            "ran": True,
            "current_version": self.current_version,
            "prior_version": self.prior_version,
            "earliest_version": self.earliest_version,
            "metrics": {
                str(v): m.to_dict() for v, m in sorted(self.metrics.items())
            },
            "transitions": {
                key: {
                    metric: dict(delta) for metric, delta in transition.items()
                }
                for key, transition in self.transitions.items()
            },
            "findings_count": self.findings_count,
            "findings": [f.to_dict() for f in self.findings],
        }


# ---------------------------------------------------------------------------
# Metric computation
# ---------------------------------------------------------------------------


def compute_metrics(
    version_dir: Path, *, body_filename: Optional[str] = None
) -> Optional[VersionMetrics]:
    """Compute the seven mechanical metrics for one version dir.

    Returns ``None`` when the resolved body file does not exist or
    cannot be read — the caller (:func:`compute_drift`) treats that as
    "this version is not usable for the drift check" and graceful-skips.
    """
    version_dir = Path(version_dir)
    if body_filename is None:
        m = _VERSION_DIR_RE.match(version_dir.name)
        slug = m.group("slug") if m else version_dir.name
        body_filename = f"{slug}.md"
    body_path = version_dir / body_filename
    if not body_path.is_file():
        return None
    try:
        text = body_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None

    m = _VERSION_DIR_RE.match(version_dir.name)
    version_num = int(m.group("n")) if m else 0

    scanned = _strip_code(text)
    word_count = len(_WORD_RE.findall(scanned))

    bold_matches = list(_BOLD_RE.finditer(scanned))
    bold_span_count = len(bold_matches)
    bold_word_count = sum(len(_WORD_RE.findall(bm.group(1))) for bm in bold_matches)
    bold_word_share = (bold_word_count / word_count * 100.0) if word_count else 0.0

    hedge_marker_count = len(_HEDGE_PHRASE_RE.findall(scanned))
    hedge_marker_count += sum(
        1 for pm in _PAREN_RE.finditer(scanned) if _CAVEAT_PAREN_RE.search(pm.group(1))
    )

    meta_commentary_count = len(_META_COMMENTARY_RE.findall(scanned))

    prose = _prose_only(text)
    sentences = [s for s in _SENTENCE_SPLIT_RE.split(prose) if s.strip()]
    if sentences:
        total_sentence_words = sum(len(_WORD_RE.findall(s)) for s in sentences)
        mean_sentence_length = total_sentence_words / len(sentences)
    else:
        mean_sentence_length = 0.0

    exhibits_dir = version_dir / "exhibits"
    exhibit_count = (
        sum(1 for p in exhibits_dir.iterdir() if p.is_file())
        if exhibits_dir.is_dir()
        else 0
    )

    return VersionMetrics(
        version=version_num,
        word_count=word_count,
        bold_span_count=bold_span_count,
        bold_word_count=bold_word_count,
        bold_word_share=bold_word_share,
        hedge_marker_count=hedge_marker_count,
        meta_commentary_count=meta_commentary_count,
        mean_sentence_length=mean_sentence_length,
        exhibit_count=exhibit_count,
    )


def _format_metric_value(metric: str, value: float) -> str:
    if metric == "bold_word_share" or metric == "mean_sentence_length":
        return f"{value:.1f}" + ("%" if metric == "bold_word_share" else "")
    return str(int(value))


def _transition(prev: VersionMetrics, curr: VersionMetrics) -> dict[str, dict]:
    """Per-metric ``{from, to, delta, increased}`` for one version transition."""
    out: dict[str, dict] = {}
    for metric in ALL_METRICS:
        a = getattr(prev, metric)
        b = getattr(curr, metric)
        out[metric] = {
            "from": a,
            "to": b,
            "delta": b - a,
            "increased": b > a,
        }
    return out


def compute_drift(
    version_dir: Path, *, body_filename: Optional[str] = None
) -> VersionDriftResult:
    """Run the version-drift check for the CURRENT version dir of a thread.

    Resolves ``<thread>.{N-1}/`` and (when available) ``<thread>.{N-2}/``
    from ``version_dir``'s own name (the ``<thread>.<N>`` naming
    convention) and its parent (the portfolio root). See the module
    docstring for the graceful-skip and finding-rule contracts.
    """
    version_dir = Path(version_dir)
    match = _VERSION_DIR_RE.match(version_dir.name)
    if not match:
        return VersionDriftResult(
            ran=False,
            reason=(
                f"version_dir name {version_dir.name!r} does not match the "
                "'<thread>.<N>' naming convention"
            ),
        )
    slug = match.group("slug")
    n = int(match.group("n"))

    if n <= 1:
        return VersionDriftResult(
            ran=False,
            reason="first version of thread (N=1); no prior version to compare",
        )

    portfolio_root = version_dir.parent
    current_metrics = compute_metrics(version_dir, body_filename=body_filename)
    if current_metrics is None:
        return VersionDriftResult(
            ran=False,
            reason=f"body file missing or unreadable in {version_dir.name}/",
        )

    prior_dir = portfolio_root / f"{slug}.{n - 1}"
    prior_metrics = (
        compute_metrics(prior_dir, body_filename=body_filename)
        if prior_dir.is_dir()
        else None
    )
    if prior_metrics is None:
        return VersionDriftResult(
            ran=False,
            reason=(
                f"prior version dir {prior_dir.name}/ not found (or its body "
                "file is missing/unreadable)"
            ),
        )

    earliest_metrics: Optional[VersionMetrics] = None
    earliest_version: Optional[int] = None
    if n >= 3:
        earliest_dir = portfolio_root / f"{slug}.{n - 2}"
        if earliest_dir.is_dir():
            candidate = compute_metrics(earliest_dir, body_filename=body_filename)
            if candidate is not None:
                earliest_metrics = candidate
                earliest_version = n - 2

    metrics_by_version: dict[int, VersionMetrics] = {
        n: current_metrics,
        n - 1: prior_metrics,
    }
    if earliest_metrics is not None:
        metrics_by_version[earliest_version] = earliest_metrics

    transitions: dict[str, dict] = {
        "prior_to_current": _transition(prior_metrics, current_metrics),
    }
    if earliest_metrics is not None:
        transitions["earliest_to_prior"] = _transition(earliest_metrics, prior_metrics)

    findings: list[DriftFinding] = []
    if earliest_metrics is not None and earliest_version is not None:
        for metric in DENSIFICATION_METRICS:
            first_leg = transitions["earliest_to_prior"][metric]
            second_leg = transitions["prior_to_current"][metric]
            if first_leg["increased"] and second_leg["increased"]:
                versions = [earliest_version, n - 1, n]
                values = [
                    getattr(earliest_metrics, metric),
                    getattr(prior_metrics, metric),
                    getattr(current_metrics, metric),
                ]
                trajectory = " → ".join(
                    _format_metric_value(metric, v) for v in values
                )
                label = _METRIC_LABELS[metric]
                findings.append(
                    DriftFinding(
                        metric=metric,
                        metric_label=label,
                        versions=versions,
                        values=values,
                        trajectory=trajectory,
                        message=(
                            f"{label} increased in both of the last two version "
                            f"transitions ({slug}.{versions[0]} → "
                            f"{slug}.{versions[1]} → {slug}.{versions[2]}): "
                            f"{trajectory}. No single review flagged this because "
                            "each pass scores the version in front of it on its "
                            "own rubric merits; the ratchet is only visible "
                            "across versions."
                        ),
                        suggested_fix=(
                            f"Cut {label} back toward the {slug}.{versions[0]} "
                            f"level ({_format_metric_value(metric, values[0])}) "
                            "rather than adding further emphasis, hedging, or "
                            "meta-commentary in this revision."
                        ),
                    )
                )

    return VersionDriftResult(
        ran=True,
        current_version=n,
        prior_version=n - 1,
        earliest_version=earliest_version,
        metrics=metrics_by_version,
        transitions=transitions,
        findings=findings,
    )


__all__ = [
    "ALL_METRICS",
    "CAVEAT_PAREN_MARKERS",
    "DENSIFICATION_METRICS",
    "FINDING_SCOPE",
    "FINDING_SEVERITY",
    "HEDGE_PHRASES",
    "META_COMMENTARY_PHRASES",
    "DriftFinding",
    "VersionDriftResult",
    "VersionMetrics",
    "compute_drift",
    "compute_metrics",
]
