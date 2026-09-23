"""Live patent-corpus clients for `anvil:ip-search` (stdlib only).

Follows the ``anvil/lib/cite.py`` precedent for external-API integration:
``urllib.request`` only, an explicit ``User-Agent``, a bounded
exponential-backoff retry, no third-party HTTP library, and **no live
network in tests** (tests inject a fake opener; see
``anvil/skills/ip-search/tests/``).

Two corpora ship:

- **PatentsView Search API** (primary) — ``https://search.patentsview.org``.
  Rich, clean JSON; granted US patents with title, abstract, date,
  inventors, and assignee in one call. Requires a free API key
  (``PATENTSVIEW_API_KEY``).
- **USPTO Open Data Portal** (secondary) — ``https://api.uspto.gov``.
  Requires ``USPTO_API_KEY``. Parsed tolerantly: the ODP payload shape has
  moved more than once, so every field read here is defensive and an
  unrecognized payload degrades rather than crashing.

**Google Patents is a documented manual fallback, not a client.** It has no
public API and scraping it is against its terms, so the no-key path emits
ready-to-click search URLs (``query.SearchQuery.google_patents_url``)
instead of fabricating reference files from unverified data.

Degradation contract (load-bearing — acceptance criterion of issue #957):
every failure mode a live corpus can present — no key configured, key
rejected, endpoint unreachable, response unparseable, zero results —
surfaces as :class:`CorpusUnavailable` or an empty hit list. **Nothing in
this module raises on the no-key path**, and the orchestrator turns a
:class:`CorpusUnavailable` into a `degraded` run that writes nothing and
prints the manual fallback.
"""

from __future__ import annotations

import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Mapping, Optional, Sequence

from .query import SearchQuery

# ---------------------------------------------------------------------------
# Errors
# ---------------------------------------------------------------------------


class CorpusUnavailable(Exception):
    """The corpus could not be queried — degrade, do not crash.

    Covers: no API key configured, a key the corpus rejected (401/403),
    network failure after retries, and a response body the adapter cannot
    parse. Callers translate this into a `degraded` run.
    """


# ---------------------------------------------------------------------------
# Hit model
# ---------------------------------------------------------------------------


@dataclass
class SearchHit:
    """One patent reference returned by a corpus.

    Field names deliberately mirror the frontmatter the prior-art critics
    parse (``title`` / ``inventors`` / ``publication_date`` / ``kind`` /
    ``summary``) plus the provenance fields issue #957 asks for
    (``patent_number`` / ``assignee`` / ``url``).
    """

    patent_number: str
    title: str
    publication_date: str = ""
    inventors: List[str] = field(default_factory=list)
    assignee: Optional[str] = None
    abstract: str = ""
    kind: str = "patent"
    url: str = ""
    source: str = ""

    def haystack(self) -> str:
        """Lowercased text a relevance note is scored against."""
        return f"{self.title}\n{self.abstract}".lower()


# ---------------------------------------------------------------------------
# API-key resolution
# ---------------------------------------------------------------------------

# Per corpus, the environment variables consulted in order. The
# ``ANVIL_``-prefixed alias exists so a consumer can scope a key to anvil
# without colliding with an unrelated tool's variable of the same name.
API_KEY_ENV: Dict[str, tuple] = {
    "patentsview": ("PATENTSVIEW_API_KEY", "ANVIL_PATENTSVIEW_API_KEY"),
    "uspto": ("USPTO_API_KEY", "ANVIL_USPTO_API_KEY"),
}

# Preference order for ``--corpus auto``.
CORPUS_ORDER = ("patentsview", "uspto")


def resolve_api_key(
    corpus: str, env: Optional[Mapping[str, str]] = None
) -> Optional[str]:
    """First non-empty API key for ``corpus`` in ``env``, else ``None``.

    Returning ``None`` (rather than raising) is the whole point: the no-key
    path is a documented, supported mode.
    """

    environ = os.environ if env is None else env
    for name in API_KEY_ENV.get(corpus, ()):
        value = (environ.get(name) or "").strip()
        if value:
            return value
    return None


def available_corpora(env: Optional[Mapping[str, str]] = None) -> List[str]:
    """Corpora with a key configured, in :data:`CORPUS_ORDER` order."""

    return [c for c in CORPUS_ORDER if resolve_api_key(c, env)]


def key_env_hint(corpus: str) -> str:
    """Human-readable "set one of these" hint for ``corpus``."""

    names = API_KEY_ENV.get(corpus, ())
    return " or ".join(names) if names else "(no key variable)"


# ---------------------------------------------------------------------------
# HTTP plumbing
# ---------------------------------------------------------------------------

_USER_AGENT = "anvil-ip-search/0.1 (https://github.com/rjwalters/anvil)"
_TIMEOUT_S = 20.0
_RETRY_DELAYS = (1.0, 2.0)

# An opener is ``(request, timeout) -> bytes``. Tests inject a fake one;
# production uses :func:`_urlopen_bytes`.
Opener = Callable[[urllib.request.Request, float], bytes]


def _urlopen_bytes(req: urllib.request.Request, timeout: float) -> bytes:
    with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
        return resp.read()


def _fetch(
    req: urllib.request.Request,
    opener: Optional[Opener] = None,
    sleep: Callable[[float], None] = time.sleep,
) -> bytes:
    """Fetch ``req`` with retry-on-transient-failure.

    4xx is a definitive answer and is not retried; 401/403 (a rejected or
    missing key) and every exhausted-retry case surface as
    :class:`CorpusUnavailable` so the caller degrades.
    """

    fetch = opener or _urlopen_bytes
    last: Optional[BaseException] = None
    for attempt in range(len(_RETRY_DELAYS) + 1):
        try:
            return fetch(req, _TIMEOUT_S)
        except urllib.error.HTTPError as exc:
            if exc.code in (401, 403):
                raise CorpusUnavailable(
                    f"HTTP {exc.code} from {req.full_url.split('?')[0]} — "
                    f"the API key was rejected or is missing the required "
                    f"scope."
                ) from exc
            if 400 <= exc.code < 500:
                raise CorpusUnavailable(
                    f"HTTP {exc.code} from {req.full_url.split('?')[0]}: "
                    f"{exc.reason}"
                ) from exc
            last = exc
        except urllib.error.URLError as exc:
            last = exc
        except OSError as exc:  # socket timeouts, DNS, TLS
            last = exc
        if attempt < len(_RETRY_DELAYS):
            sleep(_RETRY_DELAYS[attempt])
    raise CorpusUnavailable(
        f"network failure after {len(_RETRY_DELAYS) + 1} attempts against "
        f"{req.full_url.split('?')[0]}: {last}"
    )


def _json_or_degrade(raw: bytes, corpus: str) -> dict:
    try:
        data = json.loads(raw.decode("utf-8", errors="replace"))
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise CorpusUnavailable(
            f"non-JSON response from {corpus}"
        ) from exc
    if not isinstance(data, dict):
        raise CorpusUnavailable(
            f"unexpected top-level JSON type from {corpus}: "
            f"{type(data).__name__}"
        )
    return data


def _text(value: object) -> str:
    return value.strip() if isinstance(value, str) else ""


def _google_patents_url(number: str) -> str:
    """Canonical citable URL for a US publication number.

    Google Patents is used as the citable landing page (it resolves a bare
    US publication number and is stable, free, and login-less). The
    reference file also records the corpus and query that produced the hit,
    so the citation is reproducible independently of that landing page.
    """

    return f"https://patents.google.com/patent/{number}"


# ---------------------------------------------------------------------------
# PatentsView (primary)
# ---------------------------------------------------------------------------

PATENTSVIEW_ENDPOINT = "https://search.patentsview.org/api/v1/patent/"

_PATENTSVIEW_FIELDS = [
    "patent_id",
    "patent_title",
    "patent_date",
    "patent_abstract",
    "patent_type",
    "inventors.inventor_name_first",
    "inventors.inventor_name_last",
    "assignees.assignee_organization",
]


def _patentsview_request(
    query: SearchQuery, api_key: str, limit: int
) -> urllib.request.Request:
    q = {
        "_or": [
            {"_text_any": {"patent_title": query.text}},
            {"_text_any": {"patent_abstract": query.text}},
        ]
    }
    params = urllib.parse.urlencode(
        {
            "q": json.dumps(q, sort_keys=True, separators=(",", ":")),
            "f": json.dumps(_PATENTSVIEW_FIELDS, separators=(",", ":")),
            "o": json.dumps({"size": limit}, separators=(",", ":")),
            "s": json.dumps(
                [{"patent_date": "desc"}], separators=(",", ":")
            ),
        }
    )
    return urllib.request.Request(
        f"{PATENTSVIEW_ENDPOINT}?{params}",
        headers={"User-Agent": _USER_AGENT, "X-Api-Key": api_key},
    )


def _patentsview_names(record: Mapping) -> List[str]:
    names: List[str] = []
    for person in record.get("inventors") or []:
        if not isinstance(person, Mapping):
            continue
        first = _text(person.get("inventor_name_first"))
        last = _text(person.get("inventor_name_last"))
        full = " ".join(part for part in (first, last) if part)
        if full:
            names.append(full)
    return names


def _patentsview_assignee(record: Mapping) -> Optional[str]:
    for org in record.get("assignees") or []:
        if not isinstance(org, Mapping):
            continue
        name = _text(org.get("assignee_organization"))
        if name:
            return name
    return None


def _patentsview_hit(record: Mapping) -> Optional[SearchHit]:
    patent_id = _text(record.get("patent_id"))
    title = _text(record.get("patent_title"))
    if not patent_id or not title:
        return None
    number = patent_id if patent_id.upper().startswith("US") else f"US{patent_id}"
    return SearchHit(
        patent_number=number,
        title=title,
        publication_date=_text(record.get("patent_date")),
        inventors=_patentsview_names(record),
        assignee=_patentsview_assignee(record),
        abstract=_text(record.get("patent_abstract")),
        kind="patent",
        url=_google_patents_url(number),
        source="patentsview",
    )


def search_patentsview(
    query: SearchQuery,
    api_key: str,
    limit: int = 10,
    opener: Optional[Opener] = None,
    sleep: Callable[[float], None] = time.sleep,
) -> List[SearchHit]:
    """Query the PatentsView Search API for one :class:`SearchQuery`.

    Returns an empty list for a zero-result query (a legitimate outcome,
    not an error). Raises :class:`CorpusUnavailable` for every transport /
    auth / parse failure.
    """

    raw = _fetch(
        _patentsview_request(query, api_key, limit), opener=opener, sleep=sleep
    )
    data = _json_or_degrade(raw, "PatentsView")
    if data.get("error"):
        raise CorpusUnavailable(
            f"PatentsView reported an error: "
            f"{data.get('message') or data.get('error')}"
        )
    records = data.get("patents")
    if records is None:
        # Zero-hit responses have historically been reported both as an
        # empty list and as a missing key; only a non-list, non-missing
        # value is a shape we cannot read.
        if "count" in data or "total_hits" in data:
            return []
        raise CorpusUnavailable(
            "PatentsView response has no 'patents' field"
        )
    if not isinstance(records, list):
        raise CorpusUnavailable(
            f"PatentsView 'patents' field is a {type(records).__name__}, "
            f"expected a list"
        )
    hits: List[SearchHit] = []
    for record in records:
        if isinstance(record, Mapping):
            hit = _patentsview_hit(record)
            if hit:
                hits.append(hit)
    return hits


# ---------------------------------------------------------------------------
# USPTO Open Data Portal (secondary)
# ---------------------------------------------------------------------------

USPTO_ENDPOINT = "https://api.uspto.gov/api/v1/patent/applications/search"


def _uspto_request(
    query: SearchQuery, api_key: str, limit: int
) -> urllib.request.Request:
    body = json.dumps(
        {
            "q": query.text,
            "pagination": {"offset": 0, "limit": limit},
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return urllib.request.Request(
        USPTO_ENDPOINT,
        data=body,
        headers={
            "User-Agent": _USER_AGENT,
            "X-API-KEY": api_key,
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
        method="POST",
    )


def _uspto_names(meta: Mapping) -> List[str]:
    names: List[str] = []
    for person in meta.get("inventorBag") or []:
        if not isinstance(person, Mapping):
            continue
        full = _text(person.get("inventorNameText"))
        if not full:
            first = _text(person.get("firstName"))
            last = _text(person.get("lastName"))
            full = " ".join(part for part in (first, last) if part)
        if full:
            names.append(full)
    return names


def _uspto_assignee(meta: Mapping) -> Optional[str]:
    for group in ("applicantBag", "assignmentBag"):
        for org in meta.get(group) or []:
            if not isinstance(org, Mapping):
                continue
            name = _text(org.get("applicantNameText")) or _text(
                org.get("organizationName")
            )
            if name:
                return name
    return None


def _uspto_hit(record: Mapping) -> Optional[SearchHit]:
    meta = record.get("applicationMetaData")
    if not isinstance(meta, Mapping):
        meta = record if isinstance(record, Mapping) else {}
    title = _text(meta.get("inventionTitle"))
    number = _text(meta.get("patentNumber"))
    kind = "patent"
    if not number:
        # A published application (not yet granted) is still prior art.
        number = _text(meta.get("earliestPublicationNumber"))
        kind = "publication"
    if not number or not title:
        return None
    if not number.upper().startswith("US"):
        number = f"US{number}"
    date = (
        _text(meta.get("grantDate"))
        or _text(meta.get("earliestPublicationDate"))
        or _text(meta.get("filingDate"))
    )
    return SearchHit(
        patent_number=number,
        title=title,
        publication_date=date,
        inventors=_uspto_names(meta),
        assignee=_uspto_assignee(meta),
        abstract=_text(meta.get("abstractText")),
        kind=kind,
        url=_google_patents_url(number),
        source="uspto",
    )


def search_uspto(
    query: SearchQuery,
    api_key: str,
    limit: int = 10,
    opener: Optional[Opener] = None,
    sleep: Callable[[float], None] = time.sleep,
) -> List[SearchHit]:
    """Query the USPTO Open Data Portal for one :class:`SearchQuery`.

    Parsed defensively: the ODP payload shape has changed more than once,
    so an unrecognized body raises :class:`CorpusUnavailable` (degrade)
    rather than propagating a ``KeyError``.
    """

    raw = _fetch(
        _uspto_request(query, api_key, limit), opener=opener, sleep=sleep
    )
    data = _json_or_degrade(raw, "USPTO ODP")
    records = data.get("patentFileWrapperDataBag")
    if records is None:
        records = data.get("results")
    if records is None:
        if "count" in data:
            return []
        raise CorpusUnavailable(
            "USPTO ODP response has no recognizable results field "
            "('patentFileWrapperDataBag' / 'results')"
        )
    if not isinstance(records, list):
        raise CorpusUnavailable(
            "USPTO ODP results field is not a list"
        )
    hits: List[SearchHit] = []
    for record in records:
        if isinstance(record, Mapping):
            hit = _uspto_hit(record)
            if hit:
                hits.append(hit)
    return hits


# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

_SEARCHERS = {
    "patentsview": search_patentsview,
    "uspto": search_uspto,
}


def search(
    corpus: str,
    query: SearchQuery,
    api_key: str,
    limit: int = 10,
    opener: Optional[Opener] = None,
    sleep: Callable[[float], None] = time.sleep,
) -> List[SearchHit]:
    """Dispatch one query to ``corpus``."""

    searcher = _SEARCHERS.get(corpus)
    if searcher is None:
        raise CorpusUnavailable(
            f"unknown corpus {corpus!r}; known: "
            f"{', '.join(sorted(_SEARCHERS))}"
        )
    return searcher(query, api_key, limit=limit, opener=opener, sleep=sleep)


def search_all(
    corpus: str,
    queries: Sequence[SearchQuery],
    api_key: str,
    limit: int = 10,
    opener: Optional[Opener] = None,
    sleep: Callable[[float], None] = time.sleep,
) -> tuple:
    """Run every query against ``corpus``, deduplicating by patent number.

    Returns ``(hits, query_map, warnings)`` where ``query_map`` maps a
    patent number to the identifiers of the queries that surfaced it, and
    ``warnings`` collects per-query degradations. A per-query failure is
    **not** fatal: the remaining queries still run, so a partial result is
    better than no result. If EVERY query fails, the last failure is
    re-raised as :class:`CorpusUnavailable` so the caller degrades cleanly.
    """

    hits: Dict[str, SearchHit] = {}
    query_map: Dict[str, List[str]] = {}
    warnings: List[str] = []
    failures = 0
    last_error: Optional[CorpusUnavailable] = None

    for q in queries:
        try:
            found = search(
                corpus, q, api_key, limit=limit, opener=opener, sleep=sleep
            )
        except CorpusUnavailable as exc:
            failures += 1
            last_error = exc
            warnings.append(f"query {q.ident}: {exc}")
            continue
        for hit in found:
            hits.setdefault(hit.patent_number, hit)
            idents = query_map.setdefault(hit.patent_number, [])
            if q.ident not in idents:
                idents.append(q.ident)

    if queries and failures == len(queries) and last_error is not None:
        raise last_error

    ordered = [hits[num] for num in sorted(hits)]
    return ordered, query_map, warnings


__all__ = [
    "API_KEY_ENV",
    "CORPUS_ORDER",
    "CorpusUnavailable",
    "PATENTSVIEW_ENDPOINT",
    "SearchHit",
    "USPTO_ENDPOINT",
    "available_corpora",
    "key_env_hint",
    "resolve_api_key",
    "search",
    "search_all",
    "search_patentsview",
    "search_uspto",
]
