"""Doc-coverage smoke test for issue #985 (and its precursor #962).

Every deck command doc whose critic writes a sidecar-manifest file
(`findings.md` / `verdict.md` / `scoring.md` / `comments.md`) into a
`staged_sidecar` staging dir must carry a one-sentence pointer to the
sanctioned Bash-heredoc fallback for a harness that pattern-matches and
rejects those filenames on a `Write` — documented once in
``anvil/lib/snippets/critics.md`` §"Orchestrator output-file guard
collisions" and cross-referenced from each command doc's
"Non-Python-driver ordering" step.

PR #970 (closing #962) propagated this breadcrumb to 31 command files
repo-wide, including the six deck sidecar-writing critic docs below. This
test pins that coverage so a future doc edit cannot silently drop the
cross-reference from one of them.

Substring-presence only, following the precedent of
``test_additive_gate_docs.py``: no Marp render, no schema parse.
"""

from __future__ import annotations

from pathlib import Path

import pytest

_HERE = Path(__file__).resolve().parent
_COMMANDS = _HERE.parent / "commands"

CROSS_REFERENCE = "Orchestrator output-file guard collisions"

# The six deck command docs whose critic writes findings.md (or a sibling
# sidecar-manifest file) into a staged_sidecar staging dir.
SIDECAR_WRITING_DOCS = (
    "deck-review.md",
    "deck-narrative.md",
    "deck-market.md",
    "deck-design.md",
    "deck-economics.md",
    "deck-audit.md",
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
