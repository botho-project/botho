"""Tests for the opt-in pre-critic prior-art search step (issue #958 / #978).

The step is the wiring that lets the `ip-uspto` / `ip-uspto-provisional`
lifecycle run `ip-search` immediately ahead of the `priorart` critic. Its
four load-bearing properties are exactly what this module asserts:

1. **Off by default** — absent the knob the runner is never called at all
   (asserted with a runner that raises if invoked, so "no network call" is
   a structural fact rather than a documented intention), and a malformed
   config leaves it off too.
2. **Never clobbers operator work** — the step pins ``force=False``, refuses
   a ``force`` argument, and a re-run leaves hand-authored files byte-identical.
3. **Machine-fetched art is distinguishable** — the ``source:
   anvil:ip-search/…`` frontmatter marker partitions the dir mechanically.
4. **Never blocks** — degraded / error / unavailable searches are reported
   and stepped over, leaving the documented "no prior art supplied → Dim 5
   null" path intact.

Config shape matches #975's shipped contract (``auto`` / ``corpus`` /
``max`` / ``min_score``, object form only — no bare-bool shorthand),
**not** the #977 prototype's ``enabled`` / ``max_references`` / ``query``
shape this suite was adapted from. See ``prior_art_step.py``'s module
docstring for the rationale.

Every corpus interaction goes through an injected cassette opener; no test
here touches the live network.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from anvil.lib.testing import tree_hash as _tree_hash
from _ip_search_fixtures import (
    cassette_opener,
    fixed_clock,
    make_thread,
    no_sleep,
)
from _ip_search_skill_lib import orchestrate, prior_art_step

PV_ENV = {"PATENTSVIEW_API_KEY": "test-key"}

OPERATOR_REFERENCE = """---
title: "Hand-collected desk research"
inventors:
  - "A. Operator"
publication_date: "2015-02-02"
kind: "patent"
summary: "Collected by hand from a conference proceeding."
patent_number: "US9000001"
---

# US9000001 — Hand-collected desk research

Notes an operator typed, including a mention of anvil:ip-search in prose
that must NOT be mistaken for a provenance marker.
"""


def _write_config(thread: Path, payload) -> None:
    (thread / prior_art_step.CONFIG_FILENAME).write_text(
        json.dumps(payload), encoding="utf-8"
    )


def _write_operator_art(thread: Path, name: str = "operator-2015.md") -> Path:
    art = thread / "prior-art"
    art.mkdir(parents=True, exist_ok=True)
    path = art / name
    path.write_text(OPERATOR_REFERENCE, encoding="utf-8")
    return path


def _live_runner(**overrides):
    """The real orchestrator with the cassette opener bolted on."""

    def _run(thread_dir, **kwargs):
        kwargs.setdefault("env", PV_ENV)
        kwargs.setdefault("opener", cassette_opener("patentsview-thermal"))
        kwargs.setdefault("sleep", no_sleep)
        kwargs.setdefault("clock", fixed_clock)
        kwargs.update(overrides)
        return orchestrate.run(thread_dir, **kwargs)

    return _run


def _exploding_runner(*_args, **_kwargs):
    raise AssertionError(
        "the search runner must not be called when the knob is off"
    )


# ---------------------------------------------------------------------------
# 1. Off by default
# ---------------------------------------------------------------------------


def test_no_config_file_leaves_the_step_off(tmp_path):
    thread = make_thread(tmp_path)
    cfg = prior_art_step.resolve_config(thread)

    assert cfg.auto is False
    assert cfg.origin == "default"
    assert cfg.warnings == ()


def test_config_without_the_knob_leaves_the_step_off(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {"max_iterations": 7, "critics": ["review", "s112"]})

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is False
    assert cfg.origin == "default"
    assert cfg.warnings == ()


def test_disabled_step_never_calls_the_runner(tmp_path):
    """No knob ⇒ no corpus query, no API-key read, no network call."""

    thread = make_thread(tmp_path)
    result = prior_art_step.run_step(thread, runner=_exploding_runner)

    assert result.state == prior_art_step.STATE_DISABLED
    assert result.ran is False
    assert result.search is None
    assert result.written == []


def test_disabled_step_is_byte_identical_on_disk(tmp_path):
    """The knob-off path leaves the thread exactly as it found it."""

    thread = make_thread(tmp_path)
    _write_operator_art(thread)
    before = _tree_hash(thread)

    prior_art_step.run_step(thread, runner=_exploding_runner)

    assert _tree_hash(thread) == before


def test_knob_false_is_explicitly_off(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": False}})

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is False
    assert cfg.origin == "thread-config"

    result = prior_art_step.run_step(thread, runner=_exploding_runner)
    assert result.state == prior_art_step.STATE_DISABLED


def test_bare_bool_knob_is_not_a_recognized_shape(tmp_path):
    """#975's shipped contract documents the object form only.

    A bare ``{"prior_art_search": true}`` (the #977 prototype's shorthand)
    is not part of the shipped contract — it must fail safe to off, not
    silently behave like the object form.
    """

    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: True})

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is False
    assert any("must be an object" in w for w in cfg.warnings)

    result = prior_art_step.run_step(thread, runner=_exploding_runner)
    assert result.state == prior_art_step.STATE_DISABLED


def test_object_form_without_auto_key_stays_off(tmp_path):
    """Setting only ``corpus``/``max``/``min_score`` does not imply opt-in.

    Matches the shipped doc's "Only `auto: true` is required to opt in"
    literally: the object form alone does not enable the step.
    """

    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"corpus": "patentsview"}})

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is False
    assert cfg.corpus == "patentsview"


@pytest.mark.parametrize(
    "payload",
    ["{not json", '"a string"', '{"prior_art_search": "yes"}'],
)
def test_malformed_config_fails_safe_to_off(tmp_path, payload):
    """A config the parser cannot understand must never enable network use."""

    thread = make_thread(tmp_path)
    (thread / prior_art_step.CONFIG_FILENAME).write_text(payload, encoding="utf-8")

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is False
    assert cfg.warnings, "a malformed config must be reported, not silently ignored"

    result = prior_art_step.run_step(thread, runner=_exploding_runner)
    assert result.state == prior_art_step.STATE_DISABLED
    # The warning survives into the step report the operator reads.
    assert any("stays off" in w for w in result.warnings)


def test_enabled_knob_with_bad_option_still_runs_with_defaults(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(
        thread,
        {prior_art_step.KNOB_KEY: {"auto": True, "corpus": "espacenet", "max": 0}},
    )

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is True
    assert cfg.corpus == "auto"
    assert cfg.max == orchestrate.DEFAULT_MAX_REFERENCES
    assert len(cfg.warnings) == 2


# ---------------------------------------------------------------------------
# The opt-in knob (object form) and the CLI override
# ---------------------------------------------------------------------------


def test_knob_auto_true_enables_the_step(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is True
    assert cfg.origin == "thread-config"
    assert cfg.corpus == "auto"


def test_knob_object_form_carries_options(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(
        thread,
        {
            prior_art_step.KNOB_KEY: {
                "auto": True,
                "corpus": "patentsview",
                "max": 3,
                "min_score": 0,
            }
        },
    )

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is True
    assert cfg.corpus == "patentsview"
    assert cfg.max == 3
    assert cfg.min_score == 0


def test_cli_override_enables_an_unconfigured_thread(tmp_path):
    thread = make_thread(tmp_path)
    cfg = prior_art_step.resolve_config(thread, cli_enable=True)

    assert cfg.auto is True
    assert cfg.origin == "cli"


def test_cli_override_suppresses_a_configured_thread(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})

    cfg = prior_art_step.resolve_config(thread, cli_enable=False)
    assert cfg.auto is False
    assert cfg.origin == "cli"

    result = prior_art_step.run_step(
        thread, cli_enable=False, runner=_exploding_runner
    )
    assert result.state == prior_art_step.STATE_DISABLED


def test_knob_options_pass_through_to_the_runner(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(
        thread,
        {
            prior_art_step.KNOB_KEY: {
                "auto": True,
                "corpus": "patentsview",
                "max": 1,
                "min_score": 2,
            }
        },
    )
    seen = {}

    def _recording_runner(thread_dir, **kwargs):
        seen.update(kwargs)
        return orchestrate.SearchRun(status="ok", thread=Path(thread_dir).name)

    prior_art_step.run_step(thread, runner=_recording_runner)

    assert seen["corpus"] == "patentsview"
    assert seen["max_references"] == 1
    assert seen["min_score"] == 2
    assert seen["force"] is False


# ---------------------------------------------------------------------------
# Regression: a thread already opted in per #975's shipped shape
# ---------------------------------------------------------------------------


def test_pre_978_shipped_config_shape_still_opts_in(tmp_path):
    """A thread's ``.anvil.json`` written against #975's shipped docs.

    This is the exact JSON shape documented in
    ``ip-uspto-prior-art.md``'s "Opt-in pre-search" section before this
    backport — it must keep opting in afterward.
    """

    thread = make_thread(tmp_path)
    _write_config(
        thread,
        {
            "prior_art_search": {
                "auto": True,
                "corpus": "auto",
                "max": 8,
                "min_score": 1,
            }
        },
    )

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is True
    assert cfg.corpus == "auto"
    assert cfg.max == 8
    assert cfg.min_score == 1
    assert cfg.warnings == ()


# ---------------------------------------------------------------------------
# 2. Knob on: a successful search populates prior-art/
# ---------------------------------------------------------------------------


def test_enabled_step_populates_prior_art(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})

    result = prior_art_step.run_step(thread, runner=_live_runner())

    assert result.state == prior_art_step.STATE_OK
    assert result.ran is True
    written = sorted(p.name for p in result.written)
    assert written == ["jones-2018.md", "smith-2019.md"]
    # Dim 5 can now score: art is available where before there was none.
    assert result.art_available is True
    assert len(result.machine_fetched) == 2
    assert result.operator_authored == []


def test_dim5_null_path_survives_a_degraded_search(tmp_path):
    """No API key ⇒ nothing written ⇒ the critic's null-Dim-5 path, intact."""

    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})

    result = prior_art_step.run_step(thread, runner=_live_runner(env={}))

    assert result.state == prior_art_step.STATE_DEGRADED
    assert result.written == []
    assert result.art_available is False
    assert result.blocking is False
    assert not (thread / "prior-art").exists()
    assert "Dimension 5 `null`" in result.report


def test_error_and_unavailable_states_never_block(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})
    (thread / "BRIEF.md").unlink()

    errored = prior_art_step.run_step(thread, runner=_live_runner())
    assert errored.state == prior_art_step.STATE_ERROR
    assert errored.blocking is False

    def _broken_runner(*_a, **_kw):
        raise ImportError("anvil:ip-search is not installed")

    unavailable = prior_art_step.run_step(thread, runner=_broken_runner)
    assert unavailable.state == prior_art_step.STATE_UNAVAILABLE
    assert unavailable.blocking is False
    assert any("could not run" in w for w in unavailable.warnings)


# ---------------------------------------------------------------------------
# 3. Idempotence — operator-authored art is never clobbered
# ---------------------------------------------------------------------------


def test_rerun_does_not_clobber_operator_authored_art(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})
    operator = _write_operator_art(thread)
    before = operator.read_bytes()

    first = prior_art_step.run_step(thread, runner=_live_runner())
    second = prior_art_step.run_step(thread, runner=_live_runner())

    assert operator.read_bytes() == before
    assert first.state == second.state == prior_art_step.STATE_OK
    # The second pass adds nothing and rewrites nothing.
    assert second.written == []
    assert len(second.preserved) == 2


def test_rerun_is_byte_identical_over_the_whole_thread(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})
    _write_operator_art(thread)

    prior_art_step.run_step(thread, runner=_live_runner())
    after_first = _tree_hash(thread)
    prior_art_step.run_step(thread, runner=_live_runner())

    assert _tree_hash(thread) == after_first


def test_step_refuses_a_force_argument(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})

    with pytest.raises(prior_art_step.PriorArtStepError):
        prior_art_step.run_step(
            thread, runner=_live_runner(), runner_kwargs={"force": True}
        )


def test_force_in_the_knob_is_ignored_with_a_warning(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(
        thread, {prior_art_step.KNOB_KEY: {"auto": True, "force": True}}
    )

    cfg = prior_art_step.resolve_config(thread)
    assert cfg.auto is True
    assert any("force is not an accepted option" in w for w in cfg.warnings)


# ---------------------------------------------------------------------------
# 4. Provenance partition
# ---------------------------------------------------------------------------


def test_partition_separates_machine_from_operator_art(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})
    operator = _write_operator_art(thread)

    result = prior_art_step.run_step(thread, runner=_live_runner())

    assert result.operator_authored == [operator]
    assert sorted(p.name for p in result.machine_fetched) == [
        "jones-2018.md",
        "smith-2019.md",
    ]


def test_body_mention_of_the_marker_is_not_provenance(tmp_path):
    """Only the leading frontmatter block attributes a file to the tool."""

    thread = make_thread(tmp_path)
    operator = _write_operator_art(thread)

    assert "anvil:ip-search" in operator.read_text(encoding="utf-8")
    assert prior_art_step.is_machine_fetched(operator) is False


def test_machine_fetched_files_carry_the_marker(tmp_path):
    thread = make_thread(tmp_path)
    _write_config(thread, {prior_art_step.KNOB_KEY: {"auto": True}})

    prior_art_step.run_step(thread, runner=_live_runner())
    emitted = thread / "prior-art" / "smith-2019.md"

    assert prior_art_step.is_machine_fetched(emitted) is True
    assert (
        f'source: "{prior_art_step.PROVENANCE_PREFIX}/patentsview"'
        in emitted.read_text(encoding="utf-8")
    )


def test_partition_on_a_thread_with_no_prior_art_dir(tmp_path):
    thread = make_thread(tmp_path)
    machine, operator = prior_art_step.partition_prior_art(thread)
    assert machine == []
    assert operator == []


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def test_report_and_summary_name_the_next_command(tmp_path):
    thread = make_thread(tmp_path)
    result = prior_art_step.run_step(thread, runner=_exploding_runner)

    assert prior_art_step.KNOB_KEY in result.report
    assert "no network call" in result.report
    line = prior_art_step.summary_line(result, "ip-uspto-provisional")
    assert line.endswith(
        f"next: ip-uspto-provisional-prior-art {thread.name}"
    )
    assert "off" in line
