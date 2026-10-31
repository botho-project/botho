"""Doc-coverage smoke test for issue #999 (precursors #970, #962).

Every slides command doc whose critic writes a sidecar-manifest file
(`findings.md` / `verdict.md` / `scoring.md` / `comments.md`) into a
`staged_sidecar` staging dir must carry a one-sentence pointer to the
sanctioned Bash-heredoc fallback for a harness that pattern-matches and
rejects those filenames on a `Write` — documented once in
``anvil/lib/snippets/critics.md`` §"Orchestrator output-file guard
collisions" and cross-referenced from each command doc's
"Non-Python-driver ordering" step.

PR #970 (closing #962) propagated this breadcrumb to 31 command files
repo-wide, including the two slides sidecar-writing critic docs below —
already true on `main` by the time issue #999 was filed. This test pins
that coverage so a future doc edit cannot silently drop the
cross-reference from one of them, mirroring
``anvil/skills/deck/tests/test_orchestrator_guard_collision_docs.py``
(added by PR #989 for the deck skill's equivalent docs).

Substring-presence only, following the precedent of
``test_additive_gate_docs.py``: no Marp render, no schema parse.
"""

from __future__ import annotations

from pathlib import Path

import pytest

_HERE = Path(__file__).resolve().parent
_COMMANDS = _HERE.parent / "commands"

CROSS_REFERENCE = "Orchestrator output-file guard collisions"

# The slides command docs whose critic writes findings.md (or a sibling
# sidecar-manifest file) into a staged_sidecar staging dir and already
# carry the cross-reference (landed by PR #970).
SIDECAR_WRITING_DOCS = (
    "slides-review.md",
    "slides-audit.md",
)


@pytest.mark.parametrize("filename", SIDECAR_WRITING_DOCS)
def test_command_doc_cross_references_guard_collision_fallback(
    filename: str,
) -> None:
    body = (_COMMANDS / filename).read_text(encoding="utf-8")
    assert CROSS_REFERENCE in body, (
        f"{filename} should cross-reference anvil/lib/snippets/critics.md "
        f'§"{CROSS_REFERENCE}" (the sanctioned Bash-heredoc fallback for a '
        f"harness that pattern-matches and rejects a sidecar-manifest "
        f"filename on a Write) alongside its Non-Python-driver-ordering "
        f"staged_sidecar step."
    )
