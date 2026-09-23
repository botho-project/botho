"""Parse a thread ``BRIEF.md`` into its inventive-feature inventory.

The provisional / non-provisional inventor brief (see
``anvil/skills/ip-uspto-provisional/examples/acme-widget-prov/``) carries a
``## §3 — Inventive features`` section whose entries are numbered
``3.1``/``3.2``/... and usually open with a bolded label::

    ## §3 — Inventive features (the disclosure denominator)

    3.1 **Split-path excitation network.** The bridge supply is divided
    into a constant-current leg and a PTAT leg ...

That inventory is the *disclosure denominator* the ``s112`` critic already
scores against, so it is also the right denominator for a prior-art search:
one query per inventive feature, and a per-feature relevance note on every
reference the search returns.

Everything here is a pure function of the brief text — no network, no
writes, no filesystem beyond reading the brief itself. Parsing is
deliberately forgiving: a brief that does not follow the canonical section
heading still yields features (whole-document numbered-entry scan), and a
brief with no numbered entries at all degrades to a single feature derived
from the frontmatter ``title``. A brief that yields nothing usable raises
:class:`BriefFeatureError` so the caller can tell the operator to pass
``--query`` explicitly rather than silently searching for nothing.
"""

from __future__ import annotations

import re
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Optional, Sequence


class BriefFeatureError(Exception):
    """The brief could not be parsed into any usable feature."""


# ---------------------------------------------------------------------------
# Term extraction vocabulary
# ---------------------------------------------------------------------------

# Function words plus the patent-drafting boilerplate that carries no
# discriminating signal in a corpus query. Kept deliberately narrow: real
# technical adjectives ("high", "thermal", "passive") are NOT stopped —
# frequency ranking is what demotes weak terms, not this list.
_STOPWORDS = frozenset(
    """
    a about above across after again against all almost along also although
    always among an and another any anything are around as at be because been
    before behind being below beside besides between beyond both but by can
    cannot could did do does doing done down during each either else enough
    etc even ever every everything except far few for from further get gets
    give given had has have having he her here hers him his how however i if
    in inside instead into is it its itself just keep kept last least less
    let like made make makes making many may maybe me might more most much
    must my near need needs neither never next no nor not nothing now of off
    often on once one only onto or other others our out over own per perhaps
    put rather same see seen several shall she should since so some something
    still such take taken than that the their them themselves then there
    therefore these they thing things this those though through throughout
    thus to together too toward under unless until up upon us use used uses
    using usually very via was way we well were what when where whether which
    while who whom whose why will with within without would yet you your
    apparatus assembly comprising configured device embodiment embodiments
    exemplary further herein invention inventive method methods present
    preferably preferred said system systems thereof thereto whereby wherein
    accordance according aspect aspects disclosure implementation
    implementations technique techniques approach approaches
    """.split()
)

# Ordinals and bare quantity words that survive tokenization but never
# discriminate between patents.
_WEAK_TERMS = frozenset(
    """
    first second third fourth fifth sixth one two three four five six seven
    eight nine ten new novel improved general generic simple basic overall
    various certain typical typically
    """.split()
)

_MIN_TERM_LEN = 3
_DEFAULT_MAX_TERMS = 10


@dataclass
class Feature:
    """One inventive feature drawn from the brief.

    ``ident`` is the brief's own numbering (``"3.1"``), or a synthetic
    ``"q1"`` / ``"title"`` identifier for the fallback paths. ``label`` is
    the bolded heading when the brief supplies one, else a truncated first
    sentence. ``terms`` is the deterministic, ranked query vocabulary.
    """

    ident: str
    label: str
    text: str
    terms: List[str] = field(default_factory=list)

    @property
    def display(self) -> str:
        return f"{self.ident} ({self.label})" if self.label else self.ident


# ---------------------------------------------------------------------------
# Frontmatter + section slicing
# ---------------------------------------------------------------------------

_FRONTMATTER_RE = re.compile(r"\A---\s*\n(.*?)\n---\s*\n", re.DOTALL)
_TITLE_RE = re.compile(r"^title:\s*(.+?)\s*$", re.MULTILINE)

# "## §3 — Inventive features", "## 3. Inventive features",
# "## Inventive features (the disclosure denominator)" all match.
_FEATURE_HEADING_RE = re.compile(
    r"^#{1,6}\s*(?:§\s*)?3?\W*\s*inventive\s+features?\b.*$",
    re.IGNORECASE | re.MULTILINE,
)
_ANY_HEADING_RE = re.compile(r"^#{1,6}\s+\S.*$", re.MULTILINE)

# "3.1 **Label.** body", "3.1 Label — body", "- 3.1 **Label.** body"
_ENTRY_RE = re.compile(
    r"^\s*[-*]?\s*(?P<ident>\d+\.\d+)[.)]?\s+(?P<rest>\S.*)$",
)
_BOLD_LABEL_RE = re.compile(r"^\*\*(?P<label>[^*]+?)\*\*\.?\s*(?P<body>.*)$")


def _frontmatter_title(text: str) -> Optional[str]:
    m = _FRONTMATTER_RE.match(text)
    if not m:
        return None
    t = _TITLE_RE.search(m.group(1))
    if not t:
        return None
    return t.group(1).strip().strip("\"'") or None


def _strip_frontmatter(text: str) -> str:
    m = _FRONTMATTER_RE.match(text)
    return text[m.end():] if m else text


def _feature_section(body: str) -> Optional[str]:
    """Slice the ``§3 — Inventive features`` section out of ``body``.

    Returns ``None`` when the brief has no such heading (the caller then
    falls back to a whole-document numbered-entry scan).
    """

    m = _FEATURE_HEADING_RE.search(body)
    if not m:
        return None
    start = m.end()
    nxt = _ANY_HEADING_RE.search(body, start)
    return body[start: nxt.start()] if nxt else body[start:]


# ---------------------------------------------------------------------------
# Tokenization / term ranking
# ---------------------------------------------------------------------------

_MARKUP_RE = re.compile(r"[*_`\[\]()<>{}]")
_TOKEN_RE = re.compile(r"[a-z][a-z0-9]*(?:-[a-z0-9]+)*")


def tokenize(text: str) -> List[str]:
    """Lowercase ``text`` into content tokens, in document order.

    Hyphenated compounds are preserved whole (``"split-path"``); markdown
    markup, units, and bare numbers are dropped. Stopwords and weak /
    too-short tokens are filtered. Duplicates are preserved so callers can
    rank by frequency.
    """

    cleaned = _MARKUP_RE.sub(" ", text.lower())
    out: List[str] = []
    for tok in _TOKEN_RE.findall(cleaned):
        tok = tok.strip("-")
        if len(tok) < _MIN_TERM_LEN:
            continue
        if tok in _STOPWORDS or tok in _WEAK_TERMS:
            continue
        out.append(tok)
    return out


def rank_terms(
    label: str, body: str, max_terms: int = _DEFAULT_MAX_TERMS
) -> List[str]:
    """Rank a feature's query vocabulary deterministically.

    Label tokens come first, in the order the label states them — the
    bolded heading is the operator's own summary of the feature and is the
    strongest available signal. Body tokens follow, ranked by descending
    frequency then alphabetically (a total order, so two runs over the same
    brief produce byte-identical queries). The result is deduplicated and
    truncated to ``max_terms``.
    """

    ordered: List[str] = []
    seen = set()
    for tok in tokenize(label):
        if tok not in seen:
            seen.add(tok)
            ordered.append(tok)

    counts = Counter(tok for tok in tokenize(body) if tok not in seen)
    for tok, _n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])):
        ordered.append(tok)

    return ordered[:max_terms] if max_terms > 0 else ordered


# ---------------------------------------------------------------------------
# Entry parsing
# ---------------------------------------------------------------------------


def _parse_entries(section: str) -> List[tuple]:
    """Split ``section`` into ``(ident, label, body)`` triples."""

    entries: List[tuple] = []
    current_ident: Optional[str] = None
    current_label = ""
    current_body: List[str] = []

    def flush() -> None:
        if current_ident is not None:
            entries.append(
                (current_ident, current_label, "\n".join(current_body).strip())
            )

    for line in section.splitlines():
        m = _ENTRY_RE.match(line)
        if m:
            flush()
            current_ident = m.group("ident")
            rest = m.group("rest").strip()
            bold = _BOLD_LABEL_RE.match(rest)
            if bold:
                current_label = bold.group("label").strip().rstrip(".")
                current_body = [bold.group("body").strip()]
            else:
                # No bolded label: take the leading sentence as the label.
                head, sep, tail = rest.partition(". ")
                if sep and len(head) <= 90:
                    current_label = head.strip()
                    current_body = [tail.strip()]
                else:
                    current_label = ""
                    current_body = [rest]
            continue
        if current_ident is not None:
            current_body.append(line)

    flush()
    return entries


def parse_features(
    brief_text: str, max_terms: int = _DEFAULT_MAX_TERMS
) -> List[Feature]:
    """Parse ``brief_text`` into a ranked list of :class:`Feature`.

    Resolution order (each step is used only when the previous yields
    nothing):

    1. Numbered entries inside the ``§3 — Inventive features`` section.
    2. Numbered entries anywhere in the document (a brief that renamed or
       dropped the canonical heading).
    3. A single synthetic ``title`` feature from the frontmatter ``title``.

    Raises:
        BriefFeatureError: when none of the three yields a feature.
    """

    title = _frontmatter_title(brief_text)
    body = _strip_frontmatter(brief_text)

    section = _feature_section(body)
    entries = _parse_entries(section) if section is not None else []
    if not entries:
        entries = _parse_entries(body)

    features: List[Feature] = []
    for ident, label, text in entries:
        terms = rank_terms(label, text, max_terms=max_terms)
        if not terms:
            continue
        features.append(
            Feature(ident=ident, label=label, text=text, terms=terms)
        )

    if features:
        return features

    if title:
        terms = rank_terms(title, "", max_terms=max_terms)
        if terms:
            return [
                Feature(
                    ident="title",
                    label=title,
                    text=title,
                    terms=terms,
                )
            ]

    raise BriefFeatureError(
        "no inventive features could be parsed from the brief "
        "(looked for a '§3 — Inventive features' section, then any "
        "'N.M ...' numbered entries, then the frontmatter title). "
        "Pass --query \"<terms>\" to search explicitly."
    )


def load_features(
    brief_path: Path, max_terms: int = _DEFAULT_MAX_TERMS
) -> List[Feature]:
    """Read ``brief_path`` and parse it via :func:`parse_features`."""

    brief_path = Path(brief_path)
    if not brief_path.is_file():
        raise BriefFeatureError(
            f"no BRIEF.md at {brief_path} — ip-search derives its queries "
            f"from the thread's inventive-feature inventory. Either add a "
            f"BRIEF.md or pass --query \"<terms>\"."
        )
    return parse_features(
        brief_path.read_text(encoding="utf-8"), max_terms=max_terms
    )


def features_from_query(
    query: str, max_terms: int = _DEFAULT_MAX_TERMS
) -> List[Feature]:
    """Build the single synthetic feature for an operator ``--query``."""

    terms = rank_terms(query, "", max_terms=max_terms)
    if not terms:
        raise BriefFeatureError(
            f"--query {query!r} yielded no searchable terms after "
            f"stopword filtering."
        )
    return [Feature(ident="q1", label=query.strip(), text=query, terms=terms)]


def merge_terms(features: Sequence[Feature], max_terms: int = 0) -> List[str]:
    """Union of every feature's terms, in first-appearance order."""

    out: List[str] = []
    seen = set()
    for feat in features:
        for tok in feat.terms:
            if tok not in seen:
                seen.add(tok)
                out.append(tok)
    return out[:max_terms] if max_terms > 0 else out


__all__ = [
    "BriefFeatureError",
    "Feature",
    "features_from_query",
    "load_features",
    "merge_terms",
    "parse_features",
    "rank_terms",
    "tokenize",
]
