"""Brief → inventive-feature parsing and query construction (issue #957).

Covers the three-tier resolution order (canonical ``§3`` section →
whole-document numbered-entry scan → frontmatter title), the deterministic
term ranking that makes two runs over one brief build identical queries,
and the ``--query`` bypass.
"""

from __future__ import annotations

import pytest

from _ip_search_fixtures import BRIEF_TEXT, make_thread
from _ip_search_skill_lib import brief_features, query


# ---------------------------------------------------------------------------
# Section parsing
# ---------------------------------------------------------------------------


def test_parses_numbered_features_from_section_3():
    feats = brief_features.parse_features(BRIEF_TEXT)
    assert [f.ident for f in feats] == ["3.1", "3.2"]
    assert feats[0].label == "Split-path excitation network"
    assert feats[1].label == "Self-referencing offset-cancellation node"


def test_section_4_entries_are_not_treated_as_features():
    """§4 embodiments live under a later heading and must not leak in."""

    feats = brief_features.parse_features(BRIEF_TEXT)
    assert "4.1" not in [f.ident for f in feats]


def test_label_tokens_lead_the_ranked_terms():
    feats = brief_features.parse_features(BRIEF_TEXT)
    terms = feats[0].terms
    assert terms[:3] == ["split-path", "excitation", "network"]


def test_term_ranking_is_deterministic():
    a = brief_features.parse_features(BRIEF_TEXT)
    b = brief_features.parse_features(BRIEF_TEXT)
    assert [f.terms for f in a] == [f.terms for f in b]


def test_stopwords_and_patent_boilerplate_are_dropped():
    terms = brief_features.rank_terms(
        "A method and apparatus wherein the present invention",
        "said embodiment comprising a widget",
    )
    assert "method" not in terms
    assert "apparatus" not in terms
    assert "wherein" not in terms
    assert "widget" in terms


def test_max_terms_caps_the_vocabulary():
    feats = brief_features.parse_features(BRIEF_TEXT, max_terms=4)
    assert all(len(f.terms) <= 4 for f in feats)


# ---------------------------------------------------------------------------
# Fallback tiers
# ---------------------------------------------------------------------------


def test_falls_back_to_whole_document_numbered_scan():
    brief = (
        "---\ntitle: Widget\n---\n\n"
        "## Key ideas\n\n"
        "2.1 **Rotary damper.** A viscous rotary damper limits slew rate.\n"
    )
    feats = brief_features.parse_features(brief)
    assert [f.ident for f in feats] == ["2.1"]
    assert "damper" in feats[0].terms


def test_falls_back_to_frontmatter_title():
    brief = "---\ntitle: Cryogenic Valve Seat Bonding\n---\n\nProse only.\n"
    feats = brief_features.parse_features(brief)
    assert len(feats) == 1
    assert feats[0].ident == "title"
    assert "cryogenic" in feats[0].terms


def test_unparseable_brief_raises_with_actionable_message():
    with pytest.raises(brief_features.BriefFeatureError) as exc:
        brief_features.parse_features("no frontmatter, no features\n")
    assert "--query" in str(exc.value)


def test_missing_brief_file_raises(tmp_path):
    with pytest.raises(brief_features.BriefFeatureError) as exc:
        brief_features.load_features(tmp_path / "BRIEF.md")
    assert "BRIEF.md" in str(exc.value)


def test_load_features_reads_from_disk(tmp_path):
    thread = make_thread(tmp_path)
    feats = brief_features.load_features(thread / "BRIEF.md")
    assert [f.ident for f in feats] == ["3.1", "3.2"]


def test_explicit_query_builds_one_synthetic_feature():
    feats = brief_features.features_from_query(
        "piezoresistive bridge temperature compensation"
    )
    assert len(feats) == 1
    assert feats[0].ident == "q1"
    assert "piezoresistive" in feats[0].terms


def test_query_of_only_stopwords_raises():
    with pytest.raises(brief_features.BriefFeatureError):
        brief_features.features_from_query("the and of for with")


# ---------------------------------------------------------------------------
# Query construction
# ---------------------------------------------------------------------------


def test_build_queries_emits_one_per_feature_plus_union():
    feats = brief_features.parse_features(BRIEF_TEXT)
    queries = query.build_queries(feats)
    assert [q.ident for q in queries] == ["3.1", "3.2", "union"]


def test_no_union_query_for_a_single_feature():
    feats = brief_features.features_from_query("thermal compensation network")
    queries = query.build_queries(feats)
    assert [q.ident for q in queries] == ["q1"]


def test_google_patents_url_is_the_documented_manual_fallback():
    feats = brief_features.features_from_query("thermal compensation")
    url = query.build_queries(feats)[0].google_patents_url
    assert url.startswith("https://patents.google.com/?q=")
    assert "thermal" in url
    assert "compensation" in url


def test_manual_fallback_urls_cover_every_query():
    feats = brief_features.parse_features(BRIEF_TEXT)
    queries = query.build_queries(feats)
    assert len(query.manual_fallback_urls(queries)) == len(queries)
