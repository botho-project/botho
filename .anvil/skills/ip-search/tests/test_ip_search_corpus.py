"""Corpus-client tests for `anvil:ip-search` (issue #957).

**No live network.** Every test injects a fake ``opener`` returning bytes
from a cassette under ``tests/cassettes/`` (the ``anvil/lib/cite.py``
precedent). The one opt-in live smoke test at the bottom is gated on an
explicit environment variable AND a real API key, so a default ``pytest``
run — from the repo root or from this directory standalone — never reaches
the network.

The behaviours under test are the ones the issue's acceptance criteria turn
on: a real query is built against a live corpus, and every failure mode
(no key, rejected key, unreachable endpoint, malformed body, zero hits)
degrades instead of crashing.
"""

from __future__ import annotations

import json
import os
import urllib.parse

import pytest

from _ip_search_fixtures import (
    cassette_opener,
    http_error,
    json_opener,
    no_sleep,
    raising_opener,
    raw_opener,
)
from _ip_search_skill_lib import brief_features, corpus, query

QUERY = query.build_queries(
    brief_features.features_from_query("thermal compensation bridge")
)[0]


# ---------------------------------------------------------------------------
# API-key resolution / graceful no-key degradation
# ---------------------------------------------------------------------------


def test_resolve_api_key_reads_the_documented_env_vars():
    assert (
        corpus.resolve_api_key("patentsview", {"PATENTSVIEW_API_KEY": "abc"})
        == "abc"
    )
    assert corpus.resolve_api_key("uspto", {"USPTO_API_KEY": "xyz"}) == "xyz"


def test_resolve_api_key_accepts_the_anvil_prefixed_alias():
    assert (
        corpus.resolve_api_key(
            "patentsview", {"ANVIL_PATENTSVIEW_API_KEY": "abc"}
        )
        == "abc"
    )


def test_resolve_api_key_returns_none_with_no_key_and_does_not_raise():
    assert corpus.resolve_api_key("patentsview", {}) is None
    assert corpus.resolve_api_key("uspto", {}) is None


def test_blank_key_is_treated_as_absent():
    assert corpus.resolve_api_key("patentsview", {"PATENTSVIEW_API_KEY": "  "}) is None


def test_available_corpora_respects_preference_order():
    env = {"USPTO_API_KEY": "u", "PATENTSVIEW_API_KEY": "p"}
    assert corpus.available_corpora(env) == ["patentsview", "uspto"]
    assert corpus.available_corpora({}) == []


def test_key_env_hint_names_the_variables():
    hint = corpus.key_env_hint("patentsview")
    assert "PATENTSVIEW_API_KEY" in hint


# ---------------------------------------------------------------------------
# PatentsView
# ---------------------------------------------------------------------------


def test_patentsview_request_carries_key_header_and_query():
    seen = []
    corpus.search_patentsview(
        QUERY, "secret-key", limit=5, opener=cassette_opener(
            "patentsview-thermal", record=seen
        )
    )
    assert len(seen) == 1
    req = seen[0]
    assert req.full_url.startswith(corpus.PATENTSVIEW_ENDPOINT)
    assert req.get_header("X-api-key") == "secret-key"
    params = urllib.parse.parse_qs(urllib.parse.urlparse(req.full_url).query)
    q = json.loads(params["q"][0])
    assert "thermal compensation bridge" in json.dumps(q)
    assert json.loads(params["o"][0]) == {"size": 5}


def test_patentsview_maps_a_hit_to_the_critic_field_vocabulary():
    hits = corpus.search_patentsview(
        QUERY, "k", opener=cassette_opener("patentsview-thermal")
    )
    assert len(hits) == 3
    first = hits[0]
    assert first.patent_number == "US10261234"
    assert first.title.startswith("Split-path excitation network")
    assert first.publication_date == "2019-04-16"
    assert first.inventors == ["Marion Smith", "Kai Nakamura"]
    assert first.assignee == "Northline Sensors Inc"
    assert first.kind == "patent"
    assert first.source == "patentsview"
    assert first.url == "https://patents.google.com/patent/US10261234"


def test_patentsview_tolerates_a_hit_with_no_assignee():
    hits = corpus.search_patentsview(
        QUERY, "k", opener=cassette_opener("patentsview-thermal")
    )
    assert hits[2].assignee is None
    assert hits[2].inventors == ["Ada Okafor"]


def test_patentsview_zero_results_is_not_an_error():
    hits = corpus.search_patentsview(
        QUERY, "k", opener=cassette_opener("patentsview-empty")
    )
    assert hits == []


def test_patentsview_rejected_key_degrades():
    with pytest.raises(corpus.CorpusUnavailable) as exc:
        corpus.search_patentsview(
            QUERY, "bad", opener=raising_opener(http_error(403)), sleep=no_sleep
        )
    assert "403" in str(exc.value)


def test_patentsview_network_failure_degrades_after_retries():
    import urllib.error

    with pytest.raises(corpus.CorpusUnavailable) as exc:
        corpus.search_patentsview(
            QUERY,
            "k",
            opener=raising_opener(urllib.error.URLError("down")),
            sleep=no_sleep,
        )
    assert "network failure" in str(exc.value)


def test_patentsview_non_json_body_degrades():
    with pytest.raises(corpus.CorpusUnavailable):
        corpus.search_patentsview(
            QUERY, "k", opener=raw_opener(b"<html>nope</html>"), sleep=no_sleep
        )


def test_patentsview_error_flag_degrades():
    with pytest.raises(corpus.CorpusUnavailable):
        corpus.search_patentsview(
            QUERY,
            "k",
            opener=json_opener({"error": True, "message": "bad query"}),
            sleep=no_sleep,
        )


def test_patentsview_unknown_shape_degrades_rather_than_keyerrors():
    with pytest.raises(corpus.CorpusUnavailable):
        corpus.search_patentsview(
            QUERY, "k", opener=json_opener({"unexpected": []}), sleep=no_sleep
        )


def test_patentsview_records_missing_required_fields_are_skipped():
    hits = corpus.search_patentsview(
        QUERY,
        "k",
        opener=json_opener(
            {"error": False, "patents": [{"patent_id": "1"}, "junk"]}
        ),
    )
    assert hits == []


# ---------------------------------------------------------------------------
# USPTO Open Data Portal
# ---------------------------------------------------------------------------


def test_uspto_request_is_a_post_with_the_key_header():
    seen = []
    corpus.search_uspto(
        QUERY, "odp-key", limit=3, opener=cassette_opener(
            "uspto-odp-thermal", record=seen
        )
    )
    req = seen[0]
    assert req.full_url == corpus.USPTO_ENDPOINT
    assert req.get_method() == "POST"
    assert req.get_header("X-api-key") == "odp-key"
    body = json.loads(req.data.decode("utf-8"))
    assert body["q"] == QUERY.text
    assert body["pagination"]["limit"] == 3


def test_uspto_maps_granted_and_published_records():
    hits = corpus.search_uspto(
        QUERY, "k", opener=cassette_opener("uspto-odp-thermal")
    )
    assert [h.patent_number for h in hits] == ["US10555111", "US20220123456"]
    granted, published = hits
    assert granted.kind == "patent"
    assert granted.publication_date == "2020-02-04"
    assert granted.assignee == "Corvid Instruments Corp"
    assert granted.inventors == ["Lena Vasquez"]
    assert published.kind == "publication"
    assert published.inventors == ["Tomas Bergstrom"]
    assert published.assignee is None


def test_uspto_unknown_shape_degrades():
    with pytest.raises(corpus.CorpusUnavailable):
        corpus.search_uspto(
            QUERY, "k", opener=json_opener({"whatever": 1}), sleep=no_sleep
        )


def test_uspto_empty_result_set_is_not_an_error():
    hits = corpus.search_uspto(
        QUERY, "k", opener=json_opener({"count": 0})
    )
    assert hits == []


# ---------------------------------------------------------------------------
# Dispatch / multi-query aggregation
# ---------------------------------------------------------------------------


def test_search_rejects_an_unknown_corpus_by_degrading():
    with pytest.raises(corpus.CorpusUnavailable):
        corpus.search("espacenet", QUERY, "k")


def test_search_all_dedupes_and_records_which_queries_matched():
    queries = query.build_queries(
        brief_features.parse_features(
            "---\ntitle: t\n---\n\n## §3 — Inventive features\n\n"
            "3.1 **Alpha node.** alpha alpha alpha\n\n"
            "3.2 **Beta node.** beta beta beta\n"
        )
    )
    hits, query_map, warnings = corpus.search_all(
        "patentsview",
        queries,
        "k",
        opener=cassette_opener("patentsview-thermal"),
    )
    # Same cassette for every query: three unique patents, each credited to
    # all three queries.
    assert len(hits) == 3
    assert warnings == []
    assert query_map["US10261234"] == ["3.1", "3.2", "union"]


def test_search_all_survives_a_partial_query_failure():
    calls = {"n": 0}

    def flaky(request, timeout):
        calls["n"] += 1
        if calls["n"] == 1:
            raise http_error(400)
        return cassette_opener("patentsview-thermal")(request, timeout)

    queries = query.build_queries(
        brief_features.parse_features(
            "---\ntitle: t\n---\n\n## §3 — Inventive features\n\n"
            "3.1 **Alpha node.** alpha alpha\n\n"
            "3.2 **Beta node.** beta beta\n"
        )
    )
    hits, _map, warnings = corpus.search_all(
        "patentsview", queries, "k", opener=flaky, sleep=no_sleep
    )
    assert len(hits) == 3
    assert len(warnings) == 1


def test_search_all_reraises_when_every_query_fails():
    queries = query.build_queries(
        brief_features.features_from_query("alpha beta gamma")
    )
    with pytest.raises(corpus.CorpusUnavailable):
        corpus.search_all(
            "patentsview",
            queries,
            "k",
            opener=raising_opener(http_error(500)),
            sleep=no_sleep,
        )


# ---------------------------------------------------------------------------
# Opt-in live smoke test (never runs by default)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(
    os.environ.get("ANVIL_IP_SEARCH_LIVE") != "1"
    or not corpus.resolve_api_key("patentsview"),
    reason=(
        "live corpus smoke test; set ANVIL_IP_SEARCH_LIVE=1 and "
        "PATENTSVIEW_API_KEY to opt in"
    ),
)
def test_live_patentsview_smoke():
    hits = corpus.search_patentsview(
        QUERY, corpus.resolve_api_key("patentsview"), limit=3
    )
    assert all(h.patent_number.startswith("US") for h in hits)
