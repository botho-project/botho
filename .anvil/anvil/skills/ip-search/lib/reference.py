"""Render one search hit into the prior-art critics' reference contract.

The output contract is not invented here — it is the shape
``ip-uspto-prior-art`` and ``ip-uspto-provisional-prior-art`` already
document as the format they parse::

    Markdown files describing the reference (preferred): one file per
    reference, frontmatter with `title`, `inventors`, `publication_date`,
    `kind` (patent | publication | product), `summary`, `claim_text`
    (if a patent).

So those five fields are emitted **exactly**, under those names, in every
file. The extra provenance fields issue #957 asks for
(``patent_number`` / ``assignee`` / ``url`` / ``source`` / ``retrieved``)
are added alongside as a superset — a YAML mapping the critics read by key
is unaffected by keys it does not look up, and dropping them would lose the
citable-URL and assignee data the issue explicitly requires.

``claim_text`` is deliberately **omitted** rather than stubbed: neither
corpus returns claim text, and an empty ``claim_text:`` key would read as
"this patent has no claims". The body carries an explicit note instead.

Write scope (structural, not advisory): :func:`prior_art_dir` refuses to
resolve a destination under an immutable version dir
(``<thread>.{N}/``) or a critic sibling (``<thread>.{N}.<tag>/``), and
:func:`assert_write_target` refuses any file path that is not a direct
child of ``<thread>/prior-art/``.
"""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable, List, Optional, Sequence

from .brief_features import Feature
from .corpus import SearchHit


class ImmutableTargetError(Exception):
    """Refused a write into a version dir / critic sibling."""


# ``acme-widget-prov.1``, ``acme-widget-prov.12.priorart`` — the immutable
# shapes the framework's state machine owns. ``ip-search`` writes to the
# thread root's ``prior-art/`` and to nothing else, ever.
_VERSION_DIR_RE = re.compile(r"^.+\.\d+(?:\.[A-Za-z0-9_-]+)?$")

PRIOR_ART_DIRNAME = "prior-art"

DISCLAIMER = (
    "This reference was collected by `anvil:ip-search`, an automated "
    "drafting aid. It is **not** a professional or attorney prior-art "
    "clearance search, is not exhaustive, and carries no opinion on "
    "patentability, validity, or freedom to operate. Have counsel run a "
    "real search before relying on this positioning."
)


# ---------------------------------------------------------------------------
# Relevance
# ---------------------------------------------------------------------------


@dataclass
class RelevanceNote:
    """Per-feature relevance of one hit, derived mechanically.

    ``matched`` is the subset of the feature's query vocabulary that
    actually appears in the hit's title or abstract. This is deliberately a
    *mechanical* overlap, not an LLM judgment: the reference file states
    what was matched and leaves the "is this actually close art?" call to
    the prior-art critic that consumes it.
    """

    feature_id: str
    label: str
    matched: List[str] = field(default_factory=list)
    in_title: List[str] = field(default_factory=list)

    @property
    def score(self) -> int:
        return len(self.matched) + len(self.in_title)


def _contains_term(haystack: str, term: str) -> bool:
    return re.search(rf"(?<![a-z0-9]){re.escape(term)}(?![a-z0-9])", haystack) is not None


def relevance_notes(
    hit: SearchHit, features: Sequence[Feature]
) -> List[RelevanceNote]:
    """Score ``hit`` against each feature's vocabulary.

    Returned in descending score order, then by feature id, so the emitted
    table is stable across runs.
    """

    title = hit.title.lower()
    body = hit.haystack()
    notes: List[RelevanceNote] = []
    for feat in features:
        matched = [t for t in feat.terms if _contains_term(body, t)]
        in_title = [t for t in matched if _contains_term(title, t)]
        if matched:
            notes.append(
                RelevanceNote(
                    feature_id=feat.ident,
                    label=feat.label,
                    matched=matched,
                    in_title=in_title,
                )
            )
    notes.sort(key=lambda n: (-n.score, n.feature_id))
    return notes


def hit_score(notes: Sequence[RelevanceNote]) -> int:
    """Total relevance of a hit across all features."""

    return sum(n.score for n in notes)


# ---------------------------------------------------------------------------
# Reference model
# ---------------------------------------------------------------------------


@dataclass
class Reference:
    """A hit plus everything needed to write its reference file."""

    slug: str
    hit: SearchHit
    notes: List[RelevanceNote] = field(default_factory=list)
    queries: List[str] = field(default_factory=list)
    thread: str = ""
    retrieved: str = ""

    @property
    def score(self) -> int:
        return hit_score(self.notes)

    @property
    def filename(self) -> str:
        return f"{self.slug}.md"


# ---------------------------------------------------------------------------
# Slugging
# ---------------------------------------------------------------------------


def _ascii_token(value: str) -> str:
    folded = unicodedata.normalize("NFKD", value)
    folded = "".join(c for c in folded if not unicodedata.combining(c))
    folded = re.sub(r"[^A-Za-z0-9]+", "-", folded).strip("-").lower()
    return folded


def _surname(name: str) -> str:
    parts = [p for p in name.replace(",", " ").split() if p]
    return parts[-1] if parts else ""


def reference_slug(hit: SearchHit, taken: Optional[Iterable[str]] = None) -> str:
    """Deterministic, human-legible slug for a reference file.

    Shape is ``<name>-<year>`` — matching the ``smith-2019`` / ``jones-2021``
    slugs the prior-art critics' own positioning-matrix examples use — where
    ``<name>`` is the first inventor's surname, falling back to the first
    token of the assignee organization, falling back to the publication
    number. Collisions take a numeric suffix (``smith-2019-2``), so the same
    inventor in the same year never overwrites an earlier file.
    """

    used = set(taken or ())
    name = ""
    if hit.inventors:
        name = _ascii_token(_surname(hit.inventors[0]))
    if not name and hit.assignee:
        name = _ascii_token(hit.assignee.split()[0])
    if not name:
        name = _ascii_token(hit.patent_number)
    if not name:
        name = "reference"

    year = hit.publication_date[:4] if hit.publication_date[:4].isdigit() else ""
    base = f"{name}-{year}" if year else name

    if base not in used:
        return base
    n = 2
    while f"{base}-{n}" in used:
        n += 1
    return f"{base}-{n}"


# ---------------------------------------------------------------------------
# YAML emission (hand-rolled, always-quoted — no yaml dep needed)
# ---------------------------------------------------------------------------


def _yaml_str(value: str) -> str:
    """Double-quoted YAML scalar with the two escapes YAML requires."""

    collapsed = " ".join(str(value).split())
    escaped = collapsed.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def _summary_text(hit: SearchHit, limit: int = 900) -> str:
    text = " ".join((hit.abstract or "").split())
    if not text:
        return (
            f"No abstract was returned by {hit.source or 'the corpus'} for "
            f"{hit.patent_number}. Read the reference at the cited URL "
            f"before relying on this entry."
        )
    if len(text) <= limit:
        return text
    return text[: limit - 1].rstrip() + "…"


def frontmatter(ref: Reference) -> str:
    """The YAML frontmatter block, including the trailing ``---``."""

    hit = ref.hit
    lines = ["---", f"title: {_yaml_str(hit.title)}"]

    if hit.inventors:
        lines.append("inventors:")
        lines.extend(f"  - {_yaml_str(name)}" for name in hit.inventors)
    else:
        lines.append("inventors: []")

    lines.append(f"publication_date: {_yaml_str(hit.publication_date)}")
    lines.append(f"kind: {_yaml_str(hit.kind)}")
    lines.append(f"summary: {_yaml_str(_summary_text(hit))}")
    lines.append(f"patent_number: {_yaml_str(hit.patent_number)}")
    lines.append(
        f"assignee: {_yaml_str(hit.assignee)}" if hit.assignee else "assignee: null"
    )
    lines.append(f"url: {_yaml_str(hit.url)}")
    lines.append(f"source: {_yaml_str(f'anvil:ip-search/{hit.source}')}")
    if ref.retrieved:
        lines.append(f"retrieved: {_yaml_str(ref.retrieved)}")
    lines.append("---")
    return "\n".join(lines)


def _relevance_table(ref: Reference) -> List[str]:
    if not ref.notes:
        return [
            "No inventive-feature vocabulary matched this reference's title "
            "or abstract; it was returned by the corpus but scored zero "
            "overlap. Keep it only if a human read says it is close art."
        ]
    rows = [
        "| Feature | Matched terms | In title |",
        "|---|---|---|",
    ]
    for note in ref.notes:
        label = f"{note.feature_id}"
        if note.label:
            label += f" — {note.label}"
        rows.append(
            f"| {label} | {', '.join(note.matched)} | "
            f"{', '.join(note.in_title) or '—'} |"
        )
    return rows


def render_reference(ref: Reference) -> str:
    """Render the complete ``<slug>.md`` reference file."""

    hit = ref.hit
    parts: List[str] = [frontmatter(ref), ""]
    parts.append(f"# {hit.patent_number} — {hit.title}")
    parts.append("")

    parts.append("## Bibliographic data")
    parts.append("")
    parts.append(f"- **Publication number**: {hit.patent_number}")
    parts.append(f"- **Kind**: {hit.kind}")
    parts.append(
        f"- **Publication date**: {hit.publication_date or 'not reported'}"
    )
    parts.append(
        f"- **Inventors**: {', '.join(hit.inventors) if hit.inventors else 'not reported'}"
    )
    parts.append(f"- **Assignee**: {hit.assignee or 'not reported'}")
    parts.append(f"- **Cited URL**: <{hit.url}>")
    parts.append("")

    parts.append("## Summary")
    parts.append("")
    parts.append(_summary_text(hit))
    parts.append("")

    heading = f"## Relevance to `{ref.thread}`" if ref.thread else "## Relevance"
    parts.append(heading)
    parts.append("")
    parts.append(
        "Mechanical term overlap between this reference and the thread's "
        "inventive-feature inventory. This is retrieval evidence, not a "
        "positioning verdict — the prior-art critic owns the verdict."
    )
    parts.append("")
    parts.extend(_relevance_table(ref))
    parts.append("")

    parts.append("## Claim text")
    parts.append("")
    parts.append(
        "Not retrieved. Neither corpus `ip-search` queries returns claim "
        "text; if this reference proves central to positioning, pull the "
        "claims from the cited URL and paste them here (the prior-art "
        "critic reads a `claim_text` frontmatter field when one is present)."
    )
    parts.append("")

    parts.append("## Provenance")
    parts.append("")
    parts.append(f"- **Corpus**: {hit.source or 'unknown'}")
    if ref.queries:
        parts.append(f"- **Matched queries**: {', '.join(ref.queries)}")
    if ref.retrieved:
        parts.append(f"- **Retrieved**: {ref.retrieved}")
    parts.append(
        "- **Collected by**: `anvil:ip-search` (automated; never overwrites "
        "an existing reference file)"
    )
    parts.append("")

    parts.append("---")
    parts.append("")
    parts.append(DISCLAIMER)
    parts.append("")

    return "\n".join(parts)


# ---------------------------------------------------------------------------
# Write-scope guard
# ---------------------------------------------------------------------------


def is_immutable_dir(path: Path) -> bool:
    """True when ``path`` names a version dir or a critic sibling."""

    return bool(_VERSION_DIR_RE.match(Path(path).name))


def prior_art_dir(thread_dir: Path) -> Path:
    """Resolve ``<thread>/prior-art`` from a thread root.

    Raises:
        ImmutableTargetError: when ``thread_dir`` is itself a version dir
            or critic sibling (``<thread>.1/``, ``<thread>.1.priorart/``),
            or when any ancestor between the thread root and the resolved
            prior-art dir is one. This is the structural enforcement of the
            "never writes into an immutable version dir" contract — it
            fires before any file is opened.
    """

    root = Path(thread_dir).resolve()
    if is_immutable_dir(root):
        raise ImmutableTargetError(
            f"refusing to write under {root.name}/ — that is an immutable "
            f"version dir / critic sibling. Pass the THREAD root "
            f"(the dir holding BRIEF.md), not a version dir."
        )
    return root / PRIOR_ART_DIRNAME


def assert_write_target(path: Path, out_dir: Path) -> Path:
    """Assert ``path`` is a direct child of ``out_dir`` and safe to write.

    Guards against a slug that escaped its sanitizer (``../``, an absolute
    path, a nested subdir) landing a file outside ``<thread>/prior-art/``.
    """

    resolved = Path(path).resolve()
    parent = Path(out_dir).resolve()
    if resolved.parent != parent:
        raise ImmutableTargetError(
            f"refusing to write {resolved} — ip-search only ever writes "
            f"direct children of {parent}"
        )
    if parent.name != PRIOR_ART_DIRNAME:
        raise ImmutableTargetError(
            f"refusing to write into {parent} — the only permitted output "
            f"directory is <thread>/{PRIOR_ART_DIRNAME}/"
        )
    if is_immutable_dir(parent.parent):
        raise ImmutableTargetError(
            f"refusing to write under {parent.parent.name}/ — that is an "
            f"immutable version dir / critic sibling"
        )
    return resolved


__all__ = [
    "DISCLAIMER",
    "ImmutableTargetError",
    "PRIOR_ART_DIRNAME",
    "Reference",
    "RelevanceNote",
    "assert_write_target",
    "frontmatter",
    "hit_score",
    "is_immutable_dir",
    "prior_art_dir",
    "reference_slug",
    "relevance_notes",
    "render_reference",
]
