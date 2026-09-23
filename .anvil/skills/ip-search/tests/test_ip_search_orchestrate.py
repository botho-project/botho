"""End-to-end orchestration tests for `anvil:ip-search` (issue #957).

Covers the three terminal statuses (`ok` / `degraded` / `error`), the
no-overwrite rule that protects hand-annotated reference files, ranking
determinism, and the corpus-selection ladder — all with an injected
cassette opener, never the live network.
"""

from __future__ import annotations

import urllib.error

import yaml

from _ip_search_fixtures import (
    cassette_opener,
    fixed_clock,
    http_error,
    json_opener,
    make_thread,
    no_sleep,
    raising_opener,
    raw_opener,
)
from _ip_search_skill_lib import orchestrate

PV_ENV = {"PATENTSVIEW_API_KEY": "test-key"}
ODP_ENV = {"USPTO_API_KEY": "odp-key"}


def _run(thread, opener=None, env=PV_ENV, **kw):
    return orchestrate.run(
        thread,
        env=env,
        opener=opener or cassette_opener("patentsview-thermal"),
        sleep=no_sleep,
        clock=fixed_clock,
        **kw,
    )


# ---------------------------------------------------------------------------
# Happy path
# ---------------------------------------------------------------------------


def test_writes_one_markdown_file_per_reference(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread)

    assert result.status == "ok"
    assert result.success
    out = thread / "prior-art"
    written = sorted(p.name for p in out.glob("*.md"))
    # The third cassette hit (a generic digital-calibration patent) shares
    # no inventive-feature vocabulary and is dropped by the default
    # ``min_score`` floor; see ``test_min_score_zero_keeps_every_hit``.
    assert written == ["jones-2018.md", "smith-2019.md"]
    assert len(result.written) == 2


def test_written_files_parse_as_the_critics_frontmatter_contract(tmp_path):
    thread = make_thread(tmp_path)
    _run(thread)

    text = (thread / "prior-art" / "smith-2019.md").read_text(encoding="utf-8")
    _, block, _rest = text.split("---\n", 2)
    data = yaml.safe_load(block)
    assert data["title"].startswith("Split-path excitation network")
    assert data["inventors"] == ["Marion Smith", "Kai Nakamura"]
    assert data["publication_date"] == "2019-04-16"
    assert data["kind"] == "patent"
    assert data["summary"]
    assert data["url"].startswith("https://patents.google.com/patent/")


def test_ranking_puts_the_closest_art_first(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread, min_score=0)
    slugs = [r.slug for r in result.references]
    # The split-path excitation patent overlaps feature 3.1 heavily; the
    # generic digital-calibration patent overlaps nothing.
    assert slugs[0] == "smith-2019"
    assert slugs[-1] == "okafor-2012"


def test_zero_overlap_hits_are_dropped_by_default(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread)
    assert "okafor-2012" not in [r.slug for r in result.references]
    assert any("matched no inventive-feature" in w for w in result.warnings)


def test_min_score_zero_keeps_every_hit(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread, min_score=0)
    assert len(result.written) == 3
    assert not any("matched no inventive-feature" in w for w in result.warnings)


def test_report_lists_queries_references_and_the_disclaimer(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread)
    assert "## Queries" in result.report
    assert "## References" in result.report
    assert "drafting aid" in result.report
    assert "3.1" in result.report


def test_max_references_caps_the_output(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread, max_references=1)
    assert len(result.written) == 1
    assert result.references[0].slug == "smith-2019"


def test_explicit_query_bypasses_the_brief(tmp_path):
    thread = tmp_path / "no-brief"
    thread.mkdir()
    result = _run(thread, query="thermal compensation bridge")
    assert result.status == "ok"
    assert [f.ident for f in result.features] == ["q1"]
    assert result.written


def test_uspto_corpus_is_selectable(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(
        thread,
        env=ODP_ENV,
        opener=cassette_opener("uspto-odp-thermal"),
        corpus="uspto",
    )
    assert result.status == "ok"
    assert result.corpus == "uspto"
    assert {r.hit.source for r in result.references} == {"uspto"}


def test_auto_prefers_patentsview_when_both_keys_are_set(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(
        thread, env={"PATENTSVIEW_API_KEY": "p", "USPTO_API_KEY": "u"}
    )
    assert result.corpus == "patentsview"


# ---------------------------------------------------------------------------
# No-overwrite discipline
# ---------------------------------------------------------------------------


def test_an_already_collected_patent_is_skipped_not_clobbered(tmp_path):
    thread = make_thread(tmp_path)
    out = thread / "prior-art"
    out.mkdir()
    hand_written = out / "smith-2019.md"
    hand_written.write_text(
        "---\ntitle: hand-annotated\npatent_number: \"US10261234\"\n---\n"
        "Operator's own positioning note.\n",
        encoding="utf-8",
    )

    result = _run(thread)

    assert hand_written.read_text(encoding="utf-8").startswith(
        "---\ntitle: hand-annotated"
    )
    assert hand_written in result.skipped
    assert hand_written not in result.written


def test_a_hand_written_file_stating_the_number_in_prose_also_counts(tmp_path):
    """The already-collected scan reads the whole file, not just frontmatter."""

    thread = make_thread(tmp_path)
    out = thread / "prior-art"
    out.mkdir()
    note = out / "the-close-one.md"
    note.write_text(
        "# Close art\n\nSee US10261234 — split-path excitation.\n",
        encoding="utf-8",
    )

    result = _run(thread)

    assert note in result.skipped
    assert "US10261234" not in "".join(
        p.read_text(encoding="utf-8") for p in result.written
    )


def test_force_rewrites_an_already_collected_reference_in_place(tmp_path):
    thread = make_thread(tmp_path)
    out = thread / "prior-art"
    out.mkdir()
    stale = out / "smith-2019.md"
    stale.write_text(
        "---\npatent_number: \"US10261234\"\n---\nstale\n", encoding="utf-8"
    )

    result = _run(thread, force=True)

    rewritten = stale.read_text(encoding="utf-8")
    assert "stale" not in rewritten
    assert "Split-path excitation network" in rewritten
    assert result.skipped == []
    # Rewritten in place — no duplicate ``smith-2019-2.md`` was minted.
    assert not (out / "smith-2019-2.md").exists()


def test_rerun_is_idempotent_and_writes_nothing_new(tmp_path):
    thread = make_thread(tmp_path)
    first = _run(thread)
    snapshot = {
        p.name: p.read_text(encoding="utf-8")
        for p in (thread / "prior-art").glob("*.md")
    }

    second = _run(thread)

    assert second.written == []
    assert len(second.skipped) == len(first.written)
    assert {
        p.name: p.read_text(encoding="utf-8")
        for p in (thread / "prior-art").glob("*.md")
    } == snapshot


def test_an_unrelated_file_with_the_same_natural_slug_is_never_clobbered(tmp_path):
    """A different document that happens to own ``smith-2019.md``."""

    thread = make_thread(tmp_path)
    out = thread / "prior-art"
    out.mkdir()
    unrelated = out / "smith-2019.md"
    unrelated.write_text("operator's own note about something else\n", encoding="utf-8")

    # ``force`` proves the collision-avoidance is in the slugger, not in
    # the skip-if-exists branch.
    result = _run(thread, force=True)
    slugs = [r.slug for r in result.references]
    assert "smith-2019-2" in slugs
    assert unrelated.read_text(encoding="utf-8") == (
        "operator's own note about something else\n"
    )


# ---------------------------------------------------------------------------
# Graceful degradation (the headline acceptance criterion)
# ---------------------------------------------------------------------------


def test_no_api_key_degrades_without_crashing_and_writes_nothing(tmp_path):
    thread = make_thread(tmp_path)
    result = orchestrate.run(thread, env={}, clock=fixed_clock)

    assert result.status == "degraded"
    assert result.success  # documented mode, exit 0
    assert result.written == []
    assert not (thread / "prior-art").exists()
    assert result.manual_urls
    assert all(
        u.startswith("https://patents.google.com/?q=") for u in result.manual_urls
    )


def test_no_key_report_names_the_env_vars_and_the_fallback(tmp_path):
    thread = make_thread(tmp_path)
    report = orchestrate.run(thread, env={}, clock=fixed_clock).report
    assert "PATENTSVIEW_API_KEY" in report
    assert "USPTO_API_KEY" in report
    assert "Google Patents" in report
    assert "wrote nothing" in report


def test_named_corpus_without_its_key_degrades_with_a_targeted_hint(tmp_path):
    thread = make_thread(tmp_path)
    result = orchestrate.run(
        thread, corpus="uspto", env={"PATENTSVIEW_API_KEY": "p"}, clock=fixed_clock
    )
    assert result.status == "degraded"
    assert any("USPTO_API_KEY" in w for w in result.warnings)


def test_rejected_key_degrades(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread, opener=raising_opener(http_error(403)))
    assert result.status == "degraded"
    assert result.written == []
    assert any("403" in w for w in result.warnings)


def test_unreachable_endpoint_degrades(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread, opener=raising_opener(urllib.error.URLError("down")))
    assert result.status == "degraded"
    assert result.manual_urls


def test_malformed_response_degrades(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread, opener=raw_opener(b"<html/>"))
    assert result.status == "degraded"
    assert result.written == []


def test_zero_results_is_ok_with_a_warning_not_an_error(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread, opener=cassette_opener("patentsview-empty"))
    assert result.status == "ok"
    assert result.written == []
    assert any("no results" in w for w in result.warnings)


def test_records_the_corpus_could_not_map_degrade_cleanly(tmp_path):
    thread = make_thread(tmp_path)
    result = _run(thread, opener=json_opener({"surprise": True}))
    assert result.status == "degraded"


# ---------------------------------------------------------------------------
# Input errors
# ---------------------------------------------------------------------------


def test_missing_brief_without_query_is_an_error_not_a_crash(tmp_path):
    thread = tmp_path / "bare-thread"
    thread.mkdir()
    result = _run(thread)
    assert result.status == "error"
    assert not result.success
    assert any("BRIEF.md" in w for w in result.warnings)
    assert not (thread / "prior-art").exists()
