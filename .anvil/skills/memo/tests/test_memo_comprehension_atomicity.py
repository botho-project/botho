"""Atomicity tests for the comprehension critic sibling (issue #753).

The comprehension critic writes its sibling dir via the shared atomicity
primitive at ``anvil/lib/sidecar.py`` (the same primitive consumed by every
other critic-writing command across the artifact-class skills). The contract —
``staged_sidecar``'s manifest-verify-then-rename-atomically shape,
``cleanup_one_staging``'s per-critic parallel-safe sweep — has its own
dedicated test suite at ``tests/lib/test_sidecar.py``; this file is the
skill-local discovery and shape regression-test anchor for the comprehension
critic's specific use of the primitive.

The assertions:

1. ``staging_path_for(<thread>.{N}.comprehension)`` returns the expected
   leading-dot ``.tmp/`` shape.
2. A successful ``staged_sidecar`` context manager block renames the staging
   dir to the final ``<thread>.{N}.comprehension/`` name; all six required
   files make it through.
3. A mid-cycle interrupt (staging dir created without a clean context exit) is
   removed by ``cleanup_one_staging`` on the next invocation; the final-named
   dir never exists in partial form.
4. ``discover_critics`` does NOT discover the staging dir (leading-dot shape is
   invisible to the discovery glob).

Per the per-skill test filename convention (#58), this file is named
``test_memo_comprehension_atomicity.py``.
"""

from __future__ import annotations

from anvil.lib.critics import discover_critics
from anvil.lib.sidecar import (
    cleanup_one_staging,
    staged_sidecar,
    staging_path_for,
)

_REQUIRED = [
    "_review.json",
    "answers.md",
    "verdicts.md",
    "comments.md",
    "_meta.json",
    "_progress.json",
]


def test_staging_path_shape_for_comprehension(tmp_path):
    """staging_path_for produces the documented leading-dot .tmp/ shape."""
    final = tmp_path / "investment-memo.5.comprehension"
    staging = staging_path_for(final)
    # Same parent (required for POSIX rename(2) atomicity).
    assert staging.parent == final.parent
    # Leading-dot + .tmp suffix.
    assert staging.name == ".investment-memo.5.comprehension.tmp"


def test_staged_sidecar_clean_completion_renames_atomically(tmp_path):
    """A clean exit from staged_sidecar produces the final-named dir."""
    final = tmp_path / "investment-memo.5.comprehension"

    with staged_sidecar(final_dir=final, required_files=_REQUIRED) as staging:
        assert staging.name == ".investment-memo.5.comprehension.tmp"
        assert staging.exists()
        # The final-named dir does NOT exist yet — rename happens at exit.
        assert not final.exists()
        for name in _REQUIRED:
            (staging / name).write_text(
                "{}" if name.endswith(".json") else "stub"
            )

    # On clean exit: staging dir gone, final-named dir present.
    assert not staging.exists()
    assert final.exists()
    for name in _REQUIRED:
        assert (final / name).exists()


def test_cleanup_one_staging_removes_orphan_comprehension_staging(tmp_path):
    """A mid-cycle interrupt leaves a staging dir; cleanup_one_staging removes it.

    Simulates the killed-mid-run case: the staging dir exists, but the context
    manager never exited cleanly. The next invocation's cleanup_one_staging
    sweep removes the orphan; the final-named dir never exists in partial form.
    """
    final = tmp_path / "investment-memo.5.comprehension"
    staging = staging_path_for(final)

    # Manually create the orphan staging dir (no atomic rename happened).
    staging.mkdir(parents=True, exist_ok=True)
    (staging / "_review.json").write_text('{"partial": true}')

    assert staging.exists()
    assert not final.exists()

    removed = cleanup_one_staging(final)
    assert removed is True
    assert not staging.exists()
    # The final-named dir still doesn't exist — partial output never made it
    # to the discoverable name.
    assert not final.exists()


def test_cleanup_one_staging_idempotent_on_no_staging(tmp_path):
    """cleanup_one_staging is idempotent: no-op on missing staging dir."""
    final = tmp_path / "investment-memo.5.comprehension"
    removed = cleanup_one_staging(final)
    assert removed is False


def test_staging_dir_invisible_to_discover_critics(tmp_path):
    """A leading-dot .comprehension.tmp/ staging dir is NOT discovered.

    Regression guard: discover_critics requires the candidate's name to start
    with ``<slug>.`` (no leading dot allowed). The comprehension critic's
    staging dir uses the leading-dot + .tmp suffix shape and MUST NOT be
    picked up by discovery even when the matching version dir exists.
    """
    version_dir = tmp_path / "investment-memo.5"
    version_dir.mkdir()

    comp_staging = staging_path_for(tmp_path / "investment-memo.5.comprehension")
    comp_staging.mkdir(parents=True)
    (comp_staging / "_review.json").write_text('{"partial": true}')

    review_dir = tmp_path / "investment-memo.5.review"
    review_dir.mkdir()
    (review_dir / "_review.json").write_text("{}")

    siblings = discover_critics(version_dir)
    sibling_names = sorted(s.name for s in siblings)
    assert "investment-memo.5.review" in sibling_names
    for name in sibling_names:
        assert not name.startswith("."), (
            f"discover_critics should never return a leading-dot dir; got {name}"
        )
    assert ".investment-memo.5.comprehension.tmp" not in sibling_names


def test_clean_comprehension_sidecar_is_discoverable(tmp_path):
    """End-to-end: clean staged_sidecar → discover_critics finds it."""
    version_dir = tmp_path / "investment-memo.5"
    version_dir.mkdir()

    final = tmp_path / "investment-memo.5.comprehension"

    with staged_sidecar(final_dir=final, required_files=_REQUIRED) as staging:
        (staging / "_review.json").write_text("{}")
        (staging / "answers.md").write_text("# Blind-read answers")
        (staging / "verdicts.md").write_text("# Comprehension verdicts")
        (staging / "comments.md").write_text("# Comments")
        (staging / "_meta.json").write_text("{}")
        (staging / "_progress.json").write_text("{}")

    siblings = discover_critics(version_dir)
    sibling_names = [s.name for s in siblings]
    assert "investment-memo.5.comprehension" in sibling_names
