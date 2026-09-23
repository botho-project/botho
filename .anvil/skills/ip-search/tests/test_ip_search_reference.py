"""Reference-file contract tests (issue #957).

The acceptance criterion these guard is "output is directly consumable by
``ip-uspto-prior-art`` / ``ip-uspto-provisional-prior-art`` with no
reformatting" — i.e. the frontmatter carries exactly the field names those
critics document (``title`` / ``inventors`` / ``publication_date`` /
``kind`` / ``summary``), parses as YAML, and the body carries the citable
URL, the per-feature relevance note, and the not-a-clearance-search
disclaimer.
"""

from __future__ import annotations

import yaml

from _ip_search_fixtures import BRIEF_TEXT
from _ip_search_skill_lib import brief_features, corpus, reference

FEATURES = brief_features.parse_features(BRIEF_TEXT)

HIT = corpus.SearchHit(
    patent_number="US10261234",
    title="Split-path excitation network for a piezoresistive bridge sensor",
    publication_date="2019-04-16",
    inventors=["Marion Smith", "Kai Nakamura"],
    assignee="Northline Sensors Inc",
    abstract=(
        "A pressure sensor includes a piezoresistive bridge driven by a "
        "split-path excitation network having a constant-current leg."
    ),
    kind="patent",
    url="https://patents.google.com/patent/US10261234",
    source="patentsview",
)


def _ref(hit=HIT, slug="smith-2019", **kw):
    notes = reference.relevance_notes(hit, FEATURES)
    return reference.Reference(
        slug=slug,
        hit=hit,
        notes=notes,
        queries=kw.pop("queries", ["3.1"]),
        thread=kw.pop("thread", "acme-widget-prov"),
        retrieved=kw.pop("retrieved", "2026-01-15"),
    )


def _frontmatter(text: str) -> dict:
    assert text.startswith("---\n")
    _, block, _rest = text.split("---\n", 2)
    return yaml.safe_load(block)


# ---------------------------------------------------------------------------
# Frontmatter contract
# ---------------------------------------------------------------------------


def test_frontmatter_is_valid_yaml_with_the_documented_critic_fields():
    data = _frontmatter(reference.render_reference(_ref()))
    for field in ("title", "inventors", "publication_date", "kind", "summary"):
        assert field in data, f"critic-documented field {field!r} missing"
    assert data["kind"] == "patent"
    assert data["inventors"] == ["Marion Smith", "Kai Nakamura"]
    assert data["publication_date"] == "2019-04-16"
    assert data["title"].startswith("Split-path excitation network")


def test_frontmatter_carries_the_issue_required_provenance_superset():
    data = _frontmatter(reference.render_reference(_ref()))
    assert data["patent_number"] == "US10261234"
    assert data["assignee"] == "Northline Sensors Inc"
    assert data["url"] == "https://patents.google.com/patent/US10261234"
    assert data["source"].startswith("anvil:ip-search/")
    assert data["retrieved"] == "2026-01-15"


def test_publication_date_stays_a_string_not_a_yaml_date():
    data = _frontmatter(reference.render_reference(_ref()))
    assert isinstance(data["publication_date"], str)


def test_claim_text_is_omitted_not_stubbed_empty():
    """An empty ``claim_text:`` would read as "this patent has no claims"."""

    data = _frontmatter(reference.render_reference(_ref()))
    assert "claim_text" not in data


def test_missing_assignee_emits_explicit_null():
    hit = corpus.SearchHit(
        patent_number="US1", title="T", assignee=None, source="patentsview"
    )
    data = _frontmatter(reference.render_reference(_ref(hit, slug="a-1")))
    assert data["assignee"] is None


def test_quotes_and_backslashes_in_a_title_survive_yaml_round_trip():
    hit = corpus.SearchHit(
        patent_number="US2",
        title='A "quoted" \\ backslash title',
        source="patentsview",
    )
    data = _frontmatter(reference.render_reference(_ref(hit, slug="b-1")))
    assert data["title"] == 'A "quoted" \\ backslash title'


def test_missing_abstract_gets_an_explicit_summary_placeholder():
    hit = corpus.SearchHit(
        patent_number="US3", title="T", abstract="", source="patentsview"
    )
    data = _frontmatter(reference.render_reference(_ref(hit, slug="c-1")))
    assert "No abstract" in data["summary"]


def test_no_inventors_emits_an_empty_list_not_a_missing_key():
    hit = corpus.SearchHit(
        patent_number="US4", title="T", inventors=[], source="patentsview"
    )
    data = _frontmatter(reference.render_reference(_ref(hit, slug="d-1")))
    assert data["inventors"] == []


# ---------------------------------------------------------------------------
# Body contract
# ---------------------------------------------------------------------------


def test_body_cites_the_url_and_the_corpus():
    text = reference.render_reference(_ref())
    assert "<https://patents.google.com/patent/US10261234>" in text
    assert "patentsview" in text


def test_body_relevance_table_ties_back_to_brief_feature_ids():
    text = reference.render_reference(_ref())
    assert "## Relevance to `acme-widget-prov`" in text
    assert "3.1 — Split-path excitation network" in text
    assert "excitation" in text


def test_disclaimer_is_always_present():
    text = reference.render_reference(_ref())
    assert "not** a professional or attorney prior-art" in text
    assert reference.DISCLAIMER in text


def test_zero_overlap_reference_says_so_instead_of_faking_a_table():
    hit = corpus.SearchHit(
        patent_number="US5",
        title="Method of baking sourdough",
        abstract="A dough is proofed.",
        source="patentsview",
    )
    ref = reference.Reference(slug="e-1", hit=hit, notes=[], thread="t")
    text = reference.render_reference(ref)
    assert "scored zero" in text


def test_rendering_is_byte_stable_across_runs():
    assert reference.render_reference(_ref()) == reference.render_reference(_ref())


# ---------------------------------------------------------------------------
# Relevance scoring
# ---------------------------------------------------------------------------


def test_relevance_notes_rank_the_matching_feature_first():
    notes = reference.relevance_notes(HIT, FEATURES)
    assert notes[0].feature_id == "3.1"
    assert "excitation" in notes[0].matched
    assert "excitation" in notes[0].in_title


def test_relevance_matching_is_whole_word():
    hit = corpus.SearchHit(
        patent_number="US6",
        title="Nonexcitationary widget",
        abstract="",
        source="patentsview",
    )
    notes = reference.relevance_notes(hit, FEATURES)
    assert all("excitation" not in n.matched for n in notes)


def test_features_with_no_overlap_are_omitted_from_the_notes():
    hit = corpus.SearchHit(
        patent_number="US7", title="A sourdough proofing box", source="patentsview"
    )
    assert reference.relevance_notes(hit, FEATURES) == []


# ---------------------------------------------------------------------------
# Slugging
# ---------------------------------------------------------------------------


def test_slug_uses_first_inventor_surname_and_year():
    assert reference.reference_slug(HIT) == "smith-2019"


def test_slug_falls_back_to_assignee_then_number():
    no_inventor = corpus.SearchHit(
        patent_number="US10261234",
        title="T",
        publication_date="2019-04-16",
        assignee="Northline Sensors Inc",
    )
    assert reference.reference_slug(no_inventor) == "northline-2019"
    bare = corpus.SearchHit(patent_number="US10261234", title="T")
    assert reference.reference_slug(bare) == "us10261234"


def test_slug_collisions_take_a_numeric_suffix():
    assert reference.reference_slug(HIT, taken=["smith-2019"]) == "smith-2019-2"
    assert (
        reference.reference_slug(HIT, taken=["smith-2019", "smith-2019-2"])
        == "smith-2019-3"
    )


def test_slug_folds_diacritics_and_punctuation():
    hit = corpus.SearchHit(
        patent_number="US8",
        title="T",
        publication_date="2021-01-01",
        inventors=["Émile Ström-Bergé"],
    )
    assert reference.reference_slug(hit) == "strom-berge-2021"
