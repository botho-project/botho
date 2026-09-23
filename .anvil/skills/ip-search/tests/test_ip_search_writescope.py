"""Write-scope tests: `anvil:ip-search` only ever writes `<thread>/prior-art/`.

The acceptance criterion is "never writes into an immutable version dir;
only into ``<thread>/prior-art/``". This suite proves that structurally
(the guard refuses before a file is opened) and empirically (a SHA-256
tree hash over everything outside ``prior-art/`` is unchanged by a full
run, the same zero-mutation discipline ``project-scout`` /
``project-photos`` use).
"""

from __future__ import annotations

from functools import partial
from pathlib import Path

import pytest

from anvil.lib.testing import tree_hash
from _ip_search_fixtures import (
    cassette_opener,
    fixed_clock,
    make_critic_sibling,
    make_thread,
    make_version_dir,
    no_sleep,
)
from _ip_search_skill_lib import orchestrate, reference

ENV = {"PATENTSVIEW_API_KEY": "test-key"}

# This suite's write-scope contract is "everything outside prior-art/ is
# untouched", so prior-art is the file-under-test's expected write target
# and stays excluded by default here (call sites override with
# exclude="__never__" when they need to prove NOTHING was written,
# including into prior-art/ itself).
_tree_hash = partial(tree_hash, exclude="prior-art")


# ---------------------------------------------------------------------------
# Structural guard
# ---------------------------------------------------------------------------


def test_prior_art_dir_resolves_under_the_thread_root(tmp_path):
    thread = make_thread(tmp_path)
    assert reference.prior_art_dir(thread) == (thread.resolve() / "prior-art")


def test_prior_art_dir_refuses_a_version_dir(tmp_path):
    version = make_version_dir(tmp_path)
    with pytest.raises(reference.ImmutableTargetError) as exc:
        reference.prior_art_dir(version)
    assert "immutable" in str(exc.value)


def test_prior_art_dir_refuses_a_critic_sibling(tmp_path):
    sibling = make_critic_sibling(tmp_path)
    with pytest.raises(reference.ImmutableTargetError):
        reference.prior_art_dir(sibling)


def test_is_immutable_dir_recognizes_both_shapes():
    assert reference.is_immutable_dir(Path("acme-widget-prov.1"))
    assert reference.is_immutable_dir(Path("acme-widget-prov.12.priorart"))
    assert not reference.is_immutable_dir(Path("acme-widget-prov"))
    assert not reference.is_immutable_dir(Path("prior-art"))


def test_assert_write_target_refuses_a_path_escaping_the_output_dir(tmp_path):
    thread = make_thread(tmp_path)
    out = reference.prior_art_dir(thread)
    out.mkdir(parents=True)
    with pytest.raises(reference.ImmutableTargetError):
        reference.assert_write_target(out / ".." / "BRIEF.md", out)


def test_assert_write_target_refuses_a_nested_subdir(tmp_path):
    thread = make_thread(tmp_path)
    out = reference.prior_art_dir(thread)
    out.mkdir(parents=True)
    with pytest.raises(reference.ImmutableTargetError):
        reference.assert_write_target(out / "sub" / "ref.md", out)


def test_assert_write_target_refuses_a_non_prior_art_directory(tmp_path):
    other = tmp_path / "refs"
    other.mkdir()
    with pytest.raises(reference.ImmutableTargetError):
        reference.assert_write_target(other / "ref.md", other)


def test_assert_write_target_accepts_a_direct_child(tmp_path):
    thread = make_thread(tmp_path)
    out = reference.prior_art_dir(thread)
    out.mkdir(parents=True)
    assert reference.assert_write_target(out / "smith-2019.md", out).name == (
        "smith-2019.md"
    )


# ---------------------------------------------------------------------------
# End-to-end write scope
# ---------------------------------------------------------------------------


def test_run_refuses_a_version_dir_as_the_thread_root(tmp_path):
    make_thread(tmp_path)
    version = make_version_dir(tmp_path)
    result = orchestrate.run(
        version,
        env=ENV,
        opener=cassette_opener("patentsview-thermal"),
        sleep=no_sleep,
        clock=fixed_clock,
    )
    assert result.status == "error"
    assert not result.success
    assert any("immutable" in w for w in result.warnings)
    assert list(version.iterdir()) == [version / "spec.tex"]


def test_run_leaves_every_sibling_dir_byte_identical(tmp_path):
    thread = make_thread(tmp_path)
    make_version_dir(tmp_path)
    make_critic_sibling(tmp_path)
    before = _tree_hash(tmp_path)

    result = orchestrate.run(
        thread,
        env=ENV,
        opener=cassette_opener("patentsview-thermal"),
        sleep=no_sleep,
        clock=fixed_clock,
    )

    assert result.status == "ok"
    assert result.written
    assert _tree_hash(tmp_path) == before


def test_every_written_file_lands_directly_in_prior_art(tmp_path):
    thread = make_thread(tmp_path)
    result = orchestrate.run(
        thread,
        env=ENV,
        opener=cassette_opener("patentsview-thermal"),
        sleep=no_sleep,
        clock=fixed_clock,
    )
    out = thread.resolve() / "prior-art"
    assert result.written
    for path in result.written:
        assert path.parent == out
        assert path.suffix == ".md"


def test_dry_run_writes_nothing_at_all(tmp_path):
    thread = make_thread(tmp_path)
    before = _tree_hash(tmp_path, exclude="__never__")

    result = orchestrate.run(
        thread,
        dry_run=True,
        env=ENV,
        opener=cassette_opener("patentsview-thermal"),
        sleep=no_sleep,
        clock=fixed_clock,
    )

    assert result.status == "ok"
    assert result.references
    assert result.written == []
    assert not (thread / "prior-art").exists()
    assert _tree_hash(tmp_path, exclude="__never__") == before
