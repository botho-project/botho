"""Single-entry orchestration for `anvil:ip-search` (issue #957).

``run(thread_dir, ...)`` composes the whole skill:

    BRIEF.md  →  inventive features  →  per-feature queries
              →  live corpus (PatentsView / USPTO ODP)
              →  ranked, deduplicated hits
              →  <thread>/prior-art/<slug>.md  (the prior-art critics' shape)

Three terminal statuses, and only one of them is an error:

- ``ok`` — at least one corpus query ran and the run completed.
- ``degraded`` — no API key configured, the key was rejected, the corpus
  was unreachable, or its response was unparseable. **Nothing is written**;
  the report carries the manual Google-Patents fallback URLs and the
  env-var hint. This is a supported mode, exits 0, and never raises.
- ``error`` — an input problem the operator must fix (no BRIEF.md and no
  ``--query``, or a destination that is an immutable version dir).

Write discipline:

- The only directory ever written is ``<thread>/prior-art/`` (enforced in
  ``reference.prior_art_dir`` / ``reference.assert_write_target``, which
  refuse a version dir or critic sibling before any file is opened).
- An existing reference file is **never overwritten** without ``force``.
  Operators hand-annotate these files (claim text, positioning notes) and a
  re-run must not silently discard that work.
- ``dry_run`` writes nothing at all.
"""

from __future__ import annotations

import re
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, List, Mapping, Optional, Sequence

from .brief_features import (
    BriefFeatureError,
    Feature,
    features_from_query,
    load_features,
)
from .corpus import (
    CORPUS_ORDER,
    CorpusUnavailable,
    SearchHit,
    available_corpora,
    key_env_hint,
    resolve_api_key,
    search_all,
)
from .query import SearchQuery, build_queries, manual_fallback_urls
from .reference import (
    ImmutableTargetError,
    Reference,
    assert_write_target,
    hit_score,
    prior_art_dir,
    reference_slug,
    relevance_notes,
    render_reference,
)

DEFAULT_MAX_REFERENCES = 8
DEFAULT_PER_QUERY_LIMIT = 10
# Minimum inventive-feature term overlap a hit must show in its title or
# abstract to be written. 0 keeps every hit the corpus returned.
DEFAULT_MIN_SCORE = 1

# US publication numbers as they appear in an emitted (or hand-written)
# reference file: ``US10261234`` (grant) / ``US20220123456`` (application).
_PUB_NUMBER_RE = re.compile(r"\bUS\d{6,}\b", re.IGNORECASE)


@dataclass
class SearchRun:
    """Everything the command doc needs to report and exit on."""

    status: str
    thread: str
    corpus: Optional[str] = None
    features: List[Feature] = field(default_factory=list)
    queries: List[SearchQuery] = field(default_factory=list)
    references: List[Reference] = field(default_factory=list)
    written: List[Path] = field(default_factory=list)
    skipped: List[Path] = field(default_factory=list)
    manual_urls: List[str] = field(default_factory=list)
    warnings: List[str] = field(default_factory=list)
    report: str = ""

    @property
    def success(self) -> bool:
        """True for ``ok`` and ``degraded`` — both exit 0.

        A degraded run is a documented outcome (no key configured), not a
        failure; only ``error`` is a nonzero exit.
        """

        return self.status in ("ok", "degraded")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _pick_corpus(
    requested: str, env: Optional[Mapping[str, str]]
) -> tuple:
    """Resolve ``(corpus, api_key, warning)`` for the requested selection.

    ``requested == "auto"`` walks :data:`CORPUS_ORDER` and takes the first
    corpus with a key. A specific corpus with no key returns
    ``(corpus, None, hint)`` so the caller degrades with a targeted message.
    """

    if requested == "auto":
        found = available_corpora(env)
        if found:
            corpus = found[0]
            return corpus, resolve_api_key(corpus, env), None
        hints = "; ".join(f"{c}: {key_env_hint(c)}" for c in CORPUS_ORDER)
        return (
            None,
            None,
            f"no patent-corpus API key found in the environment ({hints})",
        )

    key = resolve_api_key(requested, env)
    if key:
        return requested, key, None
    return (
        requested,
        None,
        f"no API key for corpus {requested!r} — set "
        f"{key_env_hint(requested)}",
    )


def _resolve_features(
    thread_dir: Path, query: Optional[str], max_terms: int
) -> List[Feature]:
    if query:
        return features_from_query(query, max_terms=max_terms)
    return load_features(thread_dir / "BRIEF.md", max_terms=max_terms)


def _rank_references(
    hits: Sequence[SearchHit],
    features: Sequence[Feature],
    query_map: Mapping[str, Sequence[str]],
    thread: str,
    retrieved: str,
    max_references: int,
    min_score: int,
    out_dir: Path,
) -> tuple:
    """Score, sort, and slug the deduplicated hits.

    Sort key is ``(-score, patent_number)`` — a total order, so a re-run
    over the same corpus response ranks identically.

    Slug assignment is collision-aware in two distinct ways, and the
    distinction matters (it is what makes a re-run idempotent rather than
    duplicating the whole prior-art dir):

    - **Same patent already collected** — an existing file in ``out_dir``
      that mentions this publication number. The reference reuses that
      file's stem, so a re-run either skips it (default) or rewrites it in
      place (``force``). It never mints ``smith-2019-2`` for a patent
      ``smith-2019.md`` already covers.
    - **Different patent, same natural slug** — e.g. a second Smith patent
      from 2019, or an unrelated hand-written ``smith-2019.md``. That takes
      the numeric suffix, so no existing file is ever clobbered.

    Hits scoring below ``min_score`` are dropped rather than written. A
    corpus indexes full text, so a hit can come back with nothing matching
    in its title or abstract — writing that into a legal artifact's
    prior-art dir costs the operator a deletion and costs the prior-art
    critic a junk row in its positioning matrix. ``min_score=0`` keeps
    everything.

    Returns ``(references, already_collected, dropped)`` where
    ``already_collected`` maps a slug to the existing path (the caller's
    skip bookkeeping) and ``dropped`` names the below-threshold hits.
    """

    scored = []
    dropped: List[str] = []
    for hit in hits:
        notes = relevance_notes(hit, features)
        score = hit_score(notes)
        if score < min_score:
            dropped.append(f"{hit.patent_number} ({hit.title})")
            continue
        scored.append((-score, hit.patent_number, hit, notes))
    scored.sort(key=lambda row: (row[0], row[1]))
    dropped.sort()

    by_number = _collected_numbers(out_dir)
    used = _existing_slugs(out_dir)
    refs: List[Reference] = []
    collected: dict = {}

    for _neg, number, hit, notes in scored[:max_references]:
        existing = by_number.get(number.upper())
        if existing is not None:
            slug = existing.stem
            collected[slug] = existing
        else:
            slug = reference_slug(hit, taken=used)
            used.append(slug)
        refs.append(
            Reference(
                slug=slug,
                hit=hit,
                notes=notes,
                queries=list(query_map.get(hit.patent_number, ())),
                thread=thread,
                retrieved=retrieved,
            )
        )
    return refs, collected, dropped


def _existing_slugs(out_dir: Path) -> List[str]:
    if not out_dir.is_dir():
        return []
    return sorted(p.stem for p in out_dir.glob("*.md"))


def _collected_numbers(out_dir: Path) -> dict:
    """Map publication number → the existing file that already covers it.

    Reads the raw text rather than only the ``patent_number`` frontmatter
    key, so a reference an operator wrote by hand (or one collected before
    the frontmatter field existed) still counts as collected as long as it
    states the number anywhere — which every usable prior-art summary does.
    """

    found: dict = {}
    if not out_dir.is_dir():
        return found
    for path in sorted(out_dir.glob("*.md")):
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for number in _PUB_NUMBER_RE.findall(text):
            found.setdefault(number.upper(), path)
    return found


def _today(clock: Callable[[], float]) -> str:
    return time.strftime("%Y-%m-%d", time.gmtime(clock()))


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------


def _render_report(run: SearchRun, out_dir: Optional[Path]) -> str:
    lines: List[str] = [f"# ip-search — {run.thread}", ""]

    lines.append(f"- **Status**: {run.status}")
    lines.append(f"- **Corpus**: {run.corpus or 'none (manual fallback)'}")
    lines.append(f"- **Features searched**: {len(run.features)}")
    lines.append(f"- **Queries issued**: {len(run.queries)}")
    if out_dir is not None:
        lines.append(f"- **Output dir**: {out_dir}")
    lines.append("")

    if run.queries:
        lines.append("## Queries")
        lines.append("")
        for q in run.queries:
            lines.append(f"- {q.describe()}")
        lines.append("")

    if run.references:
        lines.append("## References")
        lines.append("")
        lines.append("| Slug | Number | Date | Score | Title |")
        lines.append("|---|---|---|---|---|")
        for ref in run.references:
            title = ref.hit.title
            if len(title) > 70:
                title = title[:69] + "…"
            lines.append(
                f"| `{ref.slug}.md` | {ref.hit.patent_number} | "
                f"{ref.hit.publication_date or '—'} | {ref.score} | "
                f"{title} |"
            )
        lines.append("")

    if run.written:
        lines.append("## Written")
        lines.append("")
        lines.extend(f"- `{p.name}`" for p in run.written)
        lines.append("")
    if run.skipped:
        lines.append("## Skipped (already present — not overwritten)")
        lines.append("")
        lines.extend(f"- `{p.name}`" for p in run.skipped)
        lines.append("")

    if run.status == "degraded":
        lines.append("## Degraded — no reference files written")
        lines.append("")
        lines.append(
            "No live corpus was reachable, so `ip-search` wrote nothing "
            "rather than fabricating references. Configure a key and "
            "re-run, or use the manual Google Patents queries below."
        )
        lines.append("")
        lines.append("Keys read from the environment:")
        lines.append("")
        for corpus in CORPUS_ORDER:
            lines.append(f"- `{corpus}`: {key_env_hint(corpus)}")
        lines.append("")
        if run.manual_urls:
            lines.append("Manual fallback (Google Patents):")
            lines.append("")
            lines.extend(f"- <{url}>" for url in run.manual_urls)
            lines.append("")

    if run.warnings:
        lines.append("## Warnings")
        lines.append("")
        lines.extend(f"- {w}" for w in run.warnings)
        lines.append("")

    lines.append("---")
    lines.append("")
    lines.append(
        "`anvil:ip-search` is a drafting aid, not a professional or "
        "attorney clearance search. It is not exhaustive and carries no "
        "opinion on patentability, validity, or freedom to operate."
    )
    lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def run(
    thread_dir: Path,
    corpus: str = "auto",
    query: Optional[str] = None,
    max_references: int = DEFAULT_MAX_REFERENCES,
    min_score: int = DEFAULT_MIN_SCORE,
    per_query_limit: int = DEFAULT_PER_QUERY_LIMIT,
    max_terms: int = 10,
    dry_run: bool = False,
    force: bool = False,
    env: Optional[Mapping[str, str]] = None,
    opener=None,
    sleep: Callable[[float], None] = time.sleep,
    clock: Callable[[], float] = time.time,
) -> SearchRun:
    """Search patent corpora for ``thread_dir`` and write prior-art files.

    Args:
        thread_dir: the THREAD root (the dir holding ``BRIEF.md``), never a
            version dir — passing one is refused structurally.
        corpus: ``"auto"`` (first corpus with a key), ``"patentsview"``, or
            ``"uspto"``.
        query: explicit search terms, bypassing ``BRIEF.md`` parsing.
        max_references: cap on reference files emitted per run.
        min_score: minimum feature-term overlap for a hit to be written
            (``0`` keeps every corpus hit).
        per_query_limit: cap on hits requested per corpus query.
        dry_run: compute everything, write nothing.
        force: overwrite an existing ``<slug>.md`` (default: skip it).
        env / opener / sleep / clock: injection seams for tests. ``opener``
            is ``(Request, timeout) -> bytes``; no test ever touches the
            live network.

    Returns:
        A :class:`SearchRun`. Never raises for a corpus-side failure —
        those become ``status="degraded"``.
    """

    thread_path = Path(thread_dir)
    thread_name = thread_path.resolve().name

    # --- destination guard runs FIRST: refuse an immutable dir before any
    #     network call or parse work happens.
    try:
        out_dir = prior_art_dir(thread_path)
    except ImmutableTargetError as exc:
        run_state = SearchRun(status="error", thread=thread_name)
        run_state.warnings.append(str(exc))
        run_state.report = _render_report(run_state, None)
        return run_state

    # --- features
    try:
        features = _resolve_features(thread_path, query, max_terms)
    except BriefFeatureError as exc:
        run_state = SearchRun(status="error", thread=thread_name)
        run_state.warnings.append(str(exc))
        run_state.report = _render_report(run_state, out_dir)
        return run_state

    queries = build_queries(features)
    state = SearchRun(
        status="ok",
        thread=thread_name,
        features=list(features),
        queries=list(queries),
    )

    # --- corpus selection (graceful no-key degradation)
    selected, api_key, warning = _pick_corpus(corpus, env)
    state.corpus = selected
    if not api_key:
        state.status = "degraded"
        state.corpus = None
        if warning:
            state.warnings.append(warning)
        state.manual_urls = manual_fallback_urls(queries)
        state.report = _render_report(state, out_dir)
        return state

    # --- query the corpus
    try:
        hits, query_map, per_query_warnings = search_all(
            selected,
            queries,
            api_key,
            limit=per_query_limit,
            opener=opener,
            sleep=sleep,
        )
    except CorpusUnavailable as exc:
        state.status = "degraded"
        state.warnings.append(f"{selected}: {exc}")
        state.manual_urls = manual_fallback_urls(queries)
        state.report = _render_report(state, out_dir)
        return state

    state.warnings.extend(per_query_warnings)

    if not hits:
        state.warnings.append(
            f"{selected} returned no results for any of the "
            f"{len(queries)} queries — try broader --query terms, or widen "
            f"the brief's inventive-feature vocabulary."
        )
        state.report = _render_report(state, out_dir)
        return state

    # --- rank + slug (collision-safe against files already on disk)
    state.references, _already, dropped = _rank_references(
        hits,
        features,
        query_map,
        thread=thread_name,
        retrieved=_today(clock),
        max_references=max_references,
        min_score=min_score,
        out_dir=out_dir,
    )
    if dropped:
        state.warnings.append(
            f"{len(dropped)} corpus hit(s) matched no inventive-feature "
            f"vocabulary in their title or abstract and were dropped "
            f"(re-run with min_score=0 to keep them): "
            + "; ".join(dropped)
        )

    # --- write
    if dry_run:
        state.report = _render_report(state, out_dir)
        return state

    out_dir.mkdir(parents=True, exist_ok=True)
    for ref in state.references:
        target = assert_write_target(out_dir / ref.filename, out_dir)
        if target.exists() and not force:
            state.skipped.append(target)
            continue
        target.write_text(render_reference(ref), encoding="utf-8")
        state.written.append(target)

    state.report = _render_report(state, out_dir)
    return state


__all__ = [
    "DEFAULT_MAX_REFERENCES",
    "DEFAULT_MIN_SCORE",
    "DEFAULT_PER_QUERY_LIMIT",
    "SearchRun",
    "run",
]
