"""Deterministic search-query construction for `anvil:ip-search`.

One :class:`SearchQuery` per inventive feature (plus one union query across
all features, so a hit that only makes sense as a combination is still
reachable). Query construction is a pure function of the parsed features —
two runs over the same ``BRIEF.md`` build byte-identical queries, which is
what makes the emitted reference files' provenance blocks stable enough to
commit.

Nothing here touches the network; the ``google_patents_url`` a query
carries is the manual, no-key fallback the operator clicks when no API key
is configured (see ``corpus.py`` for that degradation path).
"""

from __future__ import annotations

import urllib.parse
from dataclasses import dataclass
from typing import List, Sequence

from .brief_features import Feature, merge_terms

# The union query stays small: a very long OR across every feature's
# vocabulary matches everything and ranks nothing.
_UNION_MAX_TERMS = 8


@dataclass(frozen=True)
class SearchQuery:
    """One corpus query, traceable back to the feature that produced it."""

    ident: str
    label: str
    terms: tuple

    @property
    def text(self) -> str:
        """Space-joined term string — the ``_text_any`` payload."""
        return " ".join(self.terms)

    @property
    def google_patents_url(self) -> str:
        """A ready-to-click Google Patents search URL for this query.

        Google Patents has no public API, so it is documented purely as a
        *manual* fallback: when no API key is configured, ``ip-search``
        prints these URLs instead of writing unverified reference files.
        """

        q = urllib.parse.quote_plus(self.text)
        return f"https://patents.google.com/?q={q}"

    def describe(self) -> str:
        head = f"{self.ident}"
        if self.label:
            head += f" — {self.label}"
        return f"{head}: {self.text}"


def build_queries(
    features: Sequence[Feature], include_union: bool = True
) -> List[SearchQuery]:
    """Build one query per feature, plus an optional cross-feature union.

    The union query is emitted only when there is more than one feature
    (with a single feature it would duplicate that feature's query).
    """

    queries: List[SearchQuery] = [
        SearchQuery(ident=f.ident, label=f.label, terms=tuple(f.terms))
        for f in features
        if f.terms
    ]

    if include_union and len(queries) > 1:
        union = merge_terms(features, max_terms=_UNION_MAX_TERMS)
        if union:
            queries.append(
                SearchQuery(
                    ident="union",
                    label="all inventive features",
                    terms=tuple(union),
                )
            )

    return queries


def manual_fallback_urls(queries: Sequence[SearchQuery]) -> List[str]:
    """The Google Patents URL for each query, in query order."""

    return [q.google_patents_url for q in queries]


__all__ = ["SearchQuery", "build_queries", "manual_fallback_urls"]
