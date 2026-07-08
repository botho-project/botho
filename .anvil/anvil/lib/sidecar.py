"""Atomic sidecar directory writes via staging-then-rename (issue #350).

The Studio canary surfaced 13 critic-sibling directories (memo reviews,
audits, narratives) in **partial** state after mid-cycle interrupts: some
of the expected files (`verdict.md`, `scoring.md`, `comments.md`,
`_summary.md`, `_meta.json`, `_progress.json`) made it to disk, others
did not. The existing discovery contract in :mod:`anvil.lib.critics`
(specifically ``_has_recognizable_review``) treats any sibling dir with a
canonical ``_review.json`` OR a complete legacy file triple as a
valid critic — but the partial-write shapes from interrupted sessions
slip through: e.g., a directory with ``_review.json`` only (no
``_progress.json``, no ``_meta.json``) IS discovered and IS aggregated,
silently producing under-specified sidecars.

The file-level ``tmp + os.replace`` precedent (see
:func:`anvil.lib.cite._cache_write`,
:mod:`anvil.skills.proposal.lib.synthesizer`,
:mod:`anvil.skills.project-migrate.lib.apply`,
:mod:`anvil.skills.deck.lib.imagegen`) gives correctness at the **file**
boundary; the studio failure mode is at the **directory** boundary,
where a fan-in of N file writes can be interrupted after K files.

This module provides the directory-level analog:

1. The writer stages its files into a leading-dot sibling
   ``.<slug>.<N>.<tag>.tmp/`` directory.
2. On clean completion, the staged dir is renamed (atomically, per
   POSIX ``rename(2)`` on same-filesystem dir-to-dir) to its final name
   ``<slug>.<N>.<tag>/``.
3. On exception or missing-required-file, the staged dir is **left in
   place** so a forensic check can see what was produced, and the next
   per-critic sweep removes it.
4. A per-critic entry-step sweep (:func:`cleanup_one_staging`) targets
   only the *single* staging path that corresponds to the calling
   critic's intended ``final_dir``. It is the load-bearing surface for
   the 41 wired commands across 8 skills and is **parallel-safe**:
   concurrent critics writing different sidecars under the same
   portfolio root never sweep each other's in-flight staging dirs
   (issue #376).
5. An operator-facing portfolio-wide sweep
   (:func:`cleanup_stale_staging`) walks a parent directory for
   ``*.tmp/`` leading-dot dirs and removes them, logging the count. It
   is **NOT** safe to call from a per-critic entry step in a parallel
   fan-out workflow — it will sweep sibling critics' in-flight staging
   dirs. Reserve it for operator-time maintenance (e.g. a one-shot
   portfolio orphan-cleanup pass).

The leading-dot + ``.tmp`` suffix shape is **safe from accidental
discovery** by :func:`anvil.lib.critics.discover_critics`:

- ``discover_critics`` requires the candidate's name to start with
  ``<slug>.`` (no leading dot allowed; line 125 of ``critics.py``).
- The tag segment (everything after the last dot) cannot itself contain
  a dot (line 129) — so a hypothetical non-leading-dot stage name like
  ``<slug>.<N>.review.tmp/`` is also rejected because its tag
  ``review.tmp`` carries a dot.

Either disqualifier alone is sufficient; we ship both for belt-and-
suspenders robustness.

API
---

.. code-block:: python

    from anvil.lib.sidecar import (
        staged_sidecar,
        cleanup_one_staging,
        cleanup_stale_staging,
    )

    # Per-critic entry-step sweep: targets ONLY the staging path
    # corresponding to this critic's intended final_dir. Parallel-safe
    # under fan-out workflows (issue #376).
    cleanup_one_staging(Path("output/acme-seed.3.review"))

    # Stage + atomically rename a critic sibling directory.
    with staged_sidecar(
        final_dir=Path("output/acme-seed.3.review"),
        required_files=["verdict.md", "scoring.md", "comments.md",
                        "_summary.md", "_meta.json", "_progress.json"],
    ) as staging:
        (staging / "verdict.md").write_text(...)
        (staging / "scoring.md").write_text(...)
        # ... and so on for every required file.

    # Operator-facing portfolio-wide sweep — maintenance use only, NOT
    # safe to call from a per-critic entry step in a parallel fan-out
    # workflow (see issue #376).
    cleanup_stale_staging(Path("output"))

Contract
--------

- ``staged_sidecar(final_dir, required_files)`` returns a context
  manager. The body of the ``with`` block writes files into the yielded
  staging :class:`pathlib.Path`. On clean exit, the manager verifies
  every name in ``required_files`` exists in the staging dir, then
  ``Path.rename``\\ s the staging dir to ``final_dir``. On exception in
  the body, the staging dir is left in place (no rename, no cleanup) so
  forensic checks can inspect partial state. The exception propagates.
- If ``final_dir`` already exists when ``__enter__`` runs, we raise
  :class:`FileExistsError` immediately — atomic rename only works onto a
  non-existent target, and silently overwriting would defeat the
  immutability guarantee documented in ``anvil/lib/snippets/version_layout.md``.
- ``cleanup_one_staging(final_dir)`` removes exactly the staging path
  at ``staging_path_for(final_dir)`` if it exists and matches the
  leading-dot + ``.tmp`` shape this module owns. Returns ``True`` if a
  staging dir was removed, ``False`` otherwise. Idempotent and safe to
  call repeatedly. This is the per-critic entry-step sweep — it never
  touches sibling critics' staging dirs (issue #376).
- ``cleanup_stale_staging(parent)`` walks ``parent`` for direct children
  whose name starts with ``.`` AND ends with ``.tmp`` (the leading-dot
  + suffix shape this module owns) and removes them. It is idempotent
  and safe to call repeatedly. It logs the removed-dir names at INFO
  level via the :mod:`logging` standard library. **Operator-facing
  maintenance surface only** — calling this from a per-critic entry
  step in a parallel fan-out workflow will sweep sibling critics'
  in-flight staging dirs (issue #376).

Subprocess-only by default
--------------------------

This module uses only :mod:`os`, :mod:`pathlib`, :mod:`shutil`,
:mod:`contextlib`, and :mod:`logging` from the standard library. No new
``pyproject.toml`` dependency is introduced — the
"subprocess-only-by-default" contract documented at the top of
``CLAUDE.md`` is preserved.
"""

from __future__ import annotations

import logging
import os
import shutil
from contextlib import contextmanager
from pathlib import Path
from typing import Iterable, Iterator, List, Sequence

__all__ = [
    "STAGING_SUFFIX",
    "SidecarIncompleteError",
    "staged_sidecar",
    "staging_path_for",
    "cleanup_one_staging",
    "cleanup_stale_staging",
]


# ---------------------------------------------------------------------------
# Public constants
# ---------------------------------------------------------------------------


#: The suffix appended to a final-dir name to derive its staging-dir name.
#: Combined with the leading-dot prefix in :func:`staging_path_for`, the full
#: shape is ``.<final-name>.tmp/`` (e.g., ``.acme-seed.3.review.tmp/``).
STAGING_SUFFIX = ".tmp"


#: The dotted-name prefix.
_STAGING_PREFIX = "."


_log = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


class SidecarIncompleteError(RuntimeError):
    """The staged sidecar dir is missing one or more declared
    ``required_files`` at context exit. The staging directory is left in
    place so a forensic check can inspect what WAS produced.
    """


# ---------------------------------------------------------------------------
# Path helpers
# ---------------------------------------------------------------------------


def staging_path_for(final_dir: Path) -> Path:
    """Return the staging-dir path that corresponds to ``final_dir``.

    Naming shape: a leading-dot sibling of ``final_dir`` with the
    :data:`STAGING_SUFFIX` appended. For example::

        final_dir          = Path("output/acme-seed.3.review")
        staging_path_for() = Path("output/.acme-seed.3.review.tmp")

    The staging dir lives in the same parent as the final dir — required
    for ``rename(2)`` atomicity (POSIX guarantees atomic rename only when
    source and dest are on the same filesystem; same-parent is the
    natural way to satisfy that constraint).

    Parameters
    ----------
    final_dir:
        The intended final path for the sidecar directory.

    Returns
    -------
    The staging path. Does not touch the filesystem.
    """
    final_dir = Path(final_dir)
    parent = final_dir.parent
    staging_name = f"{_STAGING_PREFIX}{final_dir.name}{STAGING_SUFFIX}"
    return parent / staging_name


# ---------------------------------------------------------------------------
# Staged sidecar context manager
# ---------------------------------------------------------------------------


@contextmanager
def staged_sidecar(
    final_dir: Path,
    required_files: Sequence[str],
    *,
    parents: bool = True,
) -> Iterator[Path]:
    """Stage a critic sidecar directory; rename atomically on completion.

    Parameters
    ----------
    final_dir:
        The intended final path for the critic sibling directory, e.g.
        ``Path("output/acme-seed.3.review")``. Must NOT already exist —
        we refuse to overwrite to preserve the immutability guarantee
        documented in ``anvil/lib/snippets/version_layout.md``.
    required_files:
        The list of file names (NOT paths — just basenames relative to
        the staging dir) that MUST exist in the staging dir at clean
        context exit. If any is missing, the rename is skipped, the
        staging dir is left in place, and :class:`SidecarIncompleteError`
        is raised. Subdirectories under the staging dir (e.g.,
        ``perspective/``) are not validated by this manifest; the
        manifest is a flat top-level-file check.
    parents:
        Forwarded to :meth:`pathlib.Path.mkdir`. When ``True`` (default)
        any missing intermediate directories are created.

    Yields
    ------
    :class:`pathlib.Path`
        The staging directory. Write all files for the sidecar into this
        directory; do NOT write into ``final_dir`` directly.

    Raises
    ------
    FileExistsError
        If ``final_dir`` already exists on entry. (We do not overwrite.)
    SidecarIncompleteError
        If clean context exit finds any name in ``required_files``
        missing from the staging dir. The staging dir is left in place
        for forensic inspection; the next per-critic invocation's
        :func:`cleanup_one_staging` call (or an operator-time
        :func:`cleanup_stale_staging` sweep) will remove it.
    Exception
        Anything raised in the context body propagates unchanged. The
        staging dir is left in place (no rename); the exception
        propagates after the staging dir is preserved.

    Examples
    --------
    Happy path::

        with staged_sidecar(
            Path("output/acme-seed.3.review"),
            required_files=["verdict.md", "scoring.md", "_progress.json"],
        ) as staging:
            (staging / "verdict.md").write_text("...")
            (staging / "scoring.md").write_text("...")
            (staging / "_progress.json").write_text("{}")
        # On block exit: staging dir is renamed to acme-seed.3.review/.

    Exception in body (e.g., a reviewer LLM error mid-write)::

        try:
            with staged_sidecar(final, required_files=[...]) as staging:
                (staging / "verdict.md").write_text("...")
                raise RuntimeError("LLM tool error")
        except RuntimeError:
            ...
        # The staging dir .<name>.tmp/ exists with verdict.md only; the
        # final dir was never created. The next entry-step
        # cleanup_one_staging(final_dir) call sweeps the .tmp/.
    """
    final_dir = Path(final_dir)
    staging = staging_path_for(final_dir)

    if final_dir.exists():
        raise FileExistsError(
            f"staged_sidecar: refusing to stage into {staging.name!r}; "
            f"final target {final_dir!s} already exists. "
            f"Caller is responsible for the resume/idempotency check "
            f"before invoking staged_sidecar."
        )

    # If a previous interrupt left a staging dir with the same name in
    # place, wipe it before we re-enter — otherwise our mkdir would fail
    # and we'd be unable to make forward progress. This is the only path
    # where staged_sidecar itself deletes; cleanup_one_staging is the
    # per-critic entry-step sweep and cleanup_stale_staging is the
    # operator-facing portfolio-wide sweep.
    if staging.exists():
        _log.info(
            "staged_sidecar: removing prior staging dir %s (interrupted "
            "previous attempt)",
            staging,
        )
        shutil.rmtree(staging)

    staging.mkdir(parents=parents, exist_ok=False)

    try:
        yield staging
    except BaseException:
        # Body errored. Leave the staging dir in place for forensics +
        # next-startup GC. Do NOT rename.
        _log.info(
            "staged_sidecar: exception in body; leaving staging dir %s "
            "for next-invocation cleanup_one_staging() sweep",
            staging,
        )
        raise

    # Clean body exit; verify the required-files manifest.
    missing = _missing_required_files(staging, required_files)
    if missing:
        _log.warning(
            "staged_sidecar: missing required files in %s: %s "
            "(staging dir left in place for forensics)",
            staging,
            ", ".join(missing),
        )
        raise SidecarIncompleteError(
            f"staged_sidecar: sidecar at {staging!s} is missing required "
            f"files: {', '.join(missing)}. The staging directory is left "
            f"in place; rename to {final_dir.name!r} has been skipped."
        )

    # Atomic rename: POSIX guarantees atomicity for same-filesystem
    # directory rename. By construction (staging_path_for places staging
    # in the same parent as final_dir) we satisfy that constraint.
    staging.rename(final_dir)


def _missing_required_files(
    staging: Path, required_files: Iterable[str]
) -> List[str]:
    """Return the subset of ``required_files`` that do not exist in
    ``staging``. Order-preserving.
    """
    missing: List[str] = []
    for name in required_files:
        if not (staging / name).exists():
            missing.append(name)
    return missing


# ---------------------------------------------------------------------------
# Per-critic entry-step sweep (parallel-safe)
# ---------------------------------------------------------------------------


def cleanup_one_staging(final_dir: Path) -> bool:
    """Remove only the staging dir corresponding to ``final_dir``.

    Computes ``staging_path_for(final_dir)`` and removes that single
    directory if (a) it exists, (b) it is a directory, and (c) its name
    matches the leading-dot + :data:`STAGING_SUFFIX` shape this module
    owns. Returns ``True`` iff a staging dir was actually removed.

    This is the **per-critic entry-step sweep** — the load-bearing
    surface for the 41 wired commands across 8 skills. It is
    **parallel-safe**: when two critics fan out concurrently under the
    same portfolio root with distinct ``final_dir`` values, each one
    targets only its own staging path and never disturbs the sibling's
    in-flight staging dir (issue #376).

    Idempotent: a second call on the same ``final_dir`` returns
    ``False`` because the first call already removed (or found absent)
    the target.

    Safe when:

    - The staging path does not exist (returns ``False``).
    - The parent directory does not exist (returns ``False``).
    - The path exists but is a file, not a directory (returns ``False``
      — this module never deletes files).
    - The staging name does not match the leading-dot + ``.tmp`` shape
      (returns ``False`` — defensive against external callers passing a
      ``final_dir`` whose derived staging name happens to be unusual).

    Logs at INFO level the removed path (one log line per removal).

    Parameters
    ----------
    final_dir:
        The intended final path for the critic sibling directory. The
        staging path is computed via :func:`staging_path_for`. The
        ``final_dir`` itself is NOT touched by this function.

    Returns
    -------
    ``True`` if a staging directory was removed, ``False`` otherwise.

    Examples
    --------
    Happy path — a leftover staging dir from a prior crash is swept::

        cleanup_one_staging(Path("output/thread.4.review"))
        # If output/.thread.4.review.tmp/ exists, it is removed.

    No-op — nothing to sweep::

        cleanup_one_staging(Path("output/thread.4.review"))
        # If output/.thread.4.review.tmp/ does not exist, returns False.

    Parallel-safety guarantee::

        # Critic A and Critic B fan out concurrently:
        cleanup_one_staging(Path("p/thread.4.perspective"))  # touches A only
        cleanup_one_staging(Path("p/thread.4.hyperlinks"))   # touches B only
        # Each call's scope is bounded to a single staging path.
    """
    final_dir = Path(final_dir)
    staging = staging_path_for(final_dir)

    if not staging.exists():
        return False
    if not staging.is_dir():
        return False
    if not _is_staging_dirname(staging.name):
        return False

    shutil.rmtree(staging)
    _log.info(
        "cleanup_one_staging: removed stale staging dir %s",
        staging,
    )
    return True


# ---------------------------------------------------------------------------
# Operator-facing portfolio-wide sweep
# ---------------------------------------------------------------------------


def cleanup_stale_staging(parent: Path) -> List[Path]:
    """Remove any leading-dot ``.tmp/`` staging dirs under ``parent``.

    Walks ``parent``'s direct children (NOT recursive — staging dirs are
    always siblings of the intended final dir, which is in the portfolio
    root) and removes every directory whose name matches the
    leading-dot + :data:`STAGING_SUFFIX` shape this module owns. Returns
    the list of removed paths (post-rmtree absolute paths) for the
    caller to log or surface in a startup message.

    Idempotent: a second call returns ``[]`` because the first call
    already deleted the matches. Safe to call on a directory that does
    not exist (returns ``[]``).

    Files (non-directories) and directories that don't match the shape
    (e.g., a hidden ``.git/`` or a non-tmp dot-dir like
    ``.archive-2026/``) are left alone.

    Logs at INFO level the count and names of removed dirs (one log line
    per call).

    **Operator-facing maintenance surface only.** This function sweeps
    *every* leading-dot ``.tmp/`` staging dir under ``parent`` — including
    staging dirs that belong to **other critics currently writing**.
    Calling this from a per-critic entry step in a parallel fan-out
    workflow (e.g. memo-perspective + memo-hyperlinks running
    concurrently) causes the second-starting critic to ``rmtree`` the
    first-starting critic's in-flight staging dir mid-write, producing
    silently-truncated final sidecars (issue #376). Per-critic entry
    steps MUST use :func:`cleanup_one_staging` instead; this function is
    reserved for operator-time portfolio-wide orphan cleanup.

    Parameters
    ----------
    parent:
        The directory to sweep. Typically the portfolio root (the
        directory that contains ``<thread>.{N}/`` version dirs and their
        critic siblings).

    Returns
    -------
    The list of removed staging-dir paths. Empty when nothing matched.
    """
    parent = Path(parent)
    if not parent.exists() or not parent.is_dir():
        return []

    removed: List[Path] = []
    for child in sorted(parent.iterdir()):
        if not child.is_dir():
            continue
        if not _is_staging_dirname(child.name):
            continue
        shutil.rmtree(child)
        removed.append(child)

    if removed:
        _log.info(
            "cleanup_stale_staging: removed %d stale .tmp staging dir(s) "
            "from %s: %s",
            len(removed),
            parent,
            ", ".join(p.name for p in removed),
        )
    return removed


def _is_staging_dirname(name: str) -> bool:
    """Return True iff ``name`` matches the leading-dot + ``.tmp`` shape
    this module owns. Conservative: we require BOTH the leading dot AND
    the trailing ``.tmp`` so we never delete an unrelated dotfile (e.g.,
    ``.git``, ``.archive-2026``).
    """
    if not name.startswith(_STAGING_PREFIX):
        return False
    if not name.endswith(STAGING_SUFFIX):
        return False
    # Require something between the dot and the .tmp — a name like
    # ".tmp" alone (no body) is suspicious and we leave it alone.
    inner = name[len(_STAGING_PREFIX) : -len(STAGING_SUFFIX)]
    return bool(inner)
