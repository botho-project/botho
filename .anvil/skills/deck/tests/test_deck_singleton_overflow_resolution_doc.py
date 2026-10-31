"""Doc-coverage tests for the singleton-class overflow resolution path
(issue #965).

Background: `deck-review`'s post-render `auto_shrink_detector` is a
**peer-relative** rule — it needs `min_peers_per_class` (default 3) pages
in a `_class:` to compute a median, so a singleton class (the one `title`
slide, the one `ask` slide) is recorded in `skipped_classes` and never
flagged. That is the deliberate D4 tautology contract pinned by
`TestF3_SingletonClassNeverFlagged` in `test_auto_shrink_detector.py`,
and it stays. Its consequence is a hole in the "unified gate" contract
(`deck-review.md` step 5c, issue #562): a source-side
`slide-content-overflow` **error** on a singleton-class slide blocks
advance with nothing post-render able to confirm or refute it, leaving
the operator with a manual look at the rendered PDF — the exact step the
gate exists to eliminate.

Issue #965 closes that hole by **documenting the sanctioned resolution**
(Option B in the issue) rather than by adding a peerless absolute-margin
signal to the detector (Option A, considered and rejected — see the
`test_review_records_why_absolute_floor_was_rejected` case below). The
named cross-check is `deck-vision`'s v1 `vertical_overflow`, which reads
the rendered PNG with a VLM: it is an automated critic pass rather than a
human hand-confirm, and it is not defeated by the background image that
saturates the pixel-bbox detector on exactly this slide shape.

Acceptance criteria asserted here:

1. `deck-review.md` step 5c names the singleton carve-out, defines the
   `uncrosscheckable_singletons` list, and states it does NOT change
   `critical_flag`.
2. `deck-review.md` records WHY an absolute-margin floor was rejected, so
   the design decision is not re-litigated.
3. `deck-review.md`'s `_summary.md` and `findings.md` templates both
   carry the new surface.
4. `deck-revise.md` step 7c carries the reviser playbook, with fixing the
   slide as the first resort and a MANDATORY vision citation on the
   escape-hatch branch.
5. `deck-revise.md`'s `_revision-log.md` worked example carries the
   resolutions table.
6. The path is discoverable from both ends — the review command that
   raises the error and the revise command that acts on it — plus the
   reciprocal pointer in `deck-vision.md`.
7. Neither command sanctions a manual look at the rendered PDF.

Substring-presence only, distinct filename
(`test_deck_singleton_overflow_resolution_doc.py`) per the #58 packaging
convention.
"""

from __future__ import annotations

from pathlib import Path

from anvil.lib.testing import read_text as _read

_HERE = Path(__file__).resolve().parent
_SKILL_ROOT = _HERE.parent  # anvil/skills/deck/
_REPO_ROOT = _HERE.parents[3]

DECK_REVIEW = _SKILL_ROOT / "commands" / "deck-review.md"
DECK_REVISE = _SKILL_ROOT / "commands" / "deck-revise.md"
DECK_VISION = _SKILL_ROOT / "commands" / "deck-vision.md"
MARP_LINT = _REPO_ROOT / "anvil" / "lib" / "marp_lint.py"


# --- deck-review.md — step 5c carve-out -------------------------------------


def test_review_step5c_names_the_singleton_carveout():
    """Step 5c must name the singleton-class hole in the unified gate.

    The gate's own contract text (`deck-review.md` step 5c) asserts the
    two lints "stand on their own"; without an explicit carve-out that
    claim is false for singleton classes and the reviewer has no
    instruction for the case.
    """
    body = _read(DECK_REVIEW)
    idx = body.find("5c. **Run silent-Marp-auto-shrink lint")
    assert idx != -1, "deck-review.md is missing the step 5c header."
    step5c = body[idx : idx + 20000]
    assert "singleton" in step5c.lower(), (
        "deck-review.md step 5c must discuss the singleton-class case "
        "(issue #965) — the one shape where the post-render half of "
        "the unified gate structurally cannot weigh in."
    )
    assert "#965" in step5c, (
        "deck-review.md step 5c must cite issue #965 so the carve-out "
        "is traceable to the canary signal that surfaced it."
    )


def test_review_defines_uncrosscheckable_singletons_intersection():
    """Step 5c must define the machine-readable list AND its predicate.

    The list is the intersection of (a) slides with a step-5b
    source-side error and (b) slides whose `_class:` is in
    `skipped_classes`. Naming only the field without the predicate
    leaves the reviewer guessing at membership.
    """
    body = _read(DECK_REVIEW)
    idx = body.find("5c. **Run silent-Marp-auto-shrink lint")
    step5c = body[idx : idx + 20000]
    assert "uncrosscheckable_singletons" in step5c, (
        "deck-review.md step 5c must define the "
        "`uncrosscheckable_singletons` list — the machine-readable "
        "handle `deck-revise` step 7c consumes."
    )
    assert "skipped_classes" in step5c, (
        "deck-review.md step 5c must name `skipped_classes` as one leg "
        "of the `uncrosscheckable_singletons` predicate."
    )
    assert "slide-content-overflow" in step5c, (
        "deck-review.md step 5c must name the source-side "
        "`slide-content-overflow` rule as the other leg of the "
        "`uncrosscheckable_singletons` predicate."
    )


def test_review_states_the_list_is_not_a_new_gate():
    """The new list must be explicitly observational.

    `critical_flag` is already true via the source-side error. If the
    doc left the list's gating status ambiguous a reviewer could
    double-count it, or a future reader could mistake a routing hint for
    a second gate.
    """
    body = _read(DECK_REVIEW)
    idx = body.find("5c. **Run silent-Marp-auto-shrink lint")
    step5c = body[idx : idx + 20000]
    assert "NOT a new gate" in step5c or "not a new gate" in step5c, (
        "deck-review.md step 5c must state that "
        "`uncrosscheckable_singletons` is NOT a new gate — "
        "`lint_critical_flag` is computed exactly as before."
    )


def test_review_names_deck_vision_v1_as_the_resolution():
    """Step 5c must name `deck-vision` v1 `vertical_overflow` explicitly.

    "Get visual confirmation" is not a sanctioned resolution; a named
    automated critic is. This is the whole substance of the fix.
    """
    body = _read(DECK_REVIEW)
    idx = body.find("5c. **Run silent-Marp-auto-shrink lint")
    step5c = body[idx : idx + 20000]
    assert "vertical_overflow" in step5c, (
        "deck-review.md step 5c must name `deck-vision`'s v1 "
        "`vertical_overflow` dimension as the sanctioned cross-check "
        "for a singleton-class overflow error."
    )
    assert "anvil-lint-disable: slide-content-overflow" in step5c, (
        "deck-review.md step 5c must name the escape hatch the vision "
        "cross-check licenses, so the reviewer knows what the "
        "resolution actually authorizes."
    )


def test_review_forbids_hand_confirming_against_the_pdf():
    """The resolution must not degenerate into a manual PDF look.

    `deck-review.md` step 5c already says reviewers should NOT
    hand-confirm against the rendered PDF; the #965 resolution has to
    honour that, not quietly reintroduce it.
    """
    body = _read(DECK_REVIEW)
    idx = body.find("5c. **Run silent-Marp-auto-shrink lint")
    step5c = body[idx : idx + 20000]
    signals = (
        "never a manual look",
        "NOT a manual look",
        "not a human hand-confirm",
        "rather than a human hand-confirm",
    )
    found = [s for s in signals if s in step5c]
    assert found, (
        "deck-review.md step 5c must state that the singleton "
        "resolution is an automated cross-check, NOT a manual look at "
        f"the rendered PDF. Expected one of {signals!r}."
    )


def test_review_records_why_absolute_floor_was_rejected():
    """The rejected Option A must be recorded with its reasons.

    Issue #965 offered an absolute-margin floor for singleton classes as
    an alternative. Recording why it was rejected keeps a future pass
    from re-litigating it — and the reasons are specific and checkable:
    the shipped singleton classes are `justify-content: center` in the
    deck CSS (large bottom margin is the intended layout), and a `bg`
    image saturates the content bbox so every margin signal reads ~0.
    """
    body = _read(DECK_REVIEW)
    idx = body.find("5c. **Run silent-Marp-auto-shrink lint")
    step5c = body[idx : idx + 20000]
    assert "absolute-margin floor" in step5c or "absolute floor" in step5c, (
        "deck-review.md step 5c must record that a peerless "
        "absolute-margin floor for singleton classes was considered "
        "(issue #965 Option A) so the decision is traceable."
    )
    assert "rejected" in step5c, (
        "deck-review.md step 5c must state that the absolute-floor "
        "signal was REJECTED, not merely deferred."
    )
    assert "justify-content: center" in step5c, (
        "deck-review.md step 5c must cite the CSS reason the absolute "
        "floor is uninformative: `section.title` / `section.ask` are "
        "`justify-content: center` in anvil-deck.css, so a large "
        "bottom margin is the intended layout, not shrink evidence."
    )
    assert "bg right:" in step5c, (
        "deck-review.md step 5c must cite the bbox-saturation reason: "
        "a `bg` / `bg right:N%` panel image paints content across the "
        "full slide height, so the pixel-bbox margin signals read ~0 "
        "and the check would answer 'clean' while blind."
    )


def test_review_preserves_the_d4_tautology_contract():
    """The peer-relative never-flag-a-singleton contract must be affirmed.

    `TestF3_SingletonClassNeverFlagged` encodes it; the doc must say it
    is unchanged so nobody 'fixes' the detector to satisfy #965.
    """
    body = _read(DECK_REVIEW)
    idx = body.find("5c. **Run silent-Marp-auto-shrink lint")
    step5c = body[idx : idx + 20000]
    assert "TestF3_SingletonClassNeverFlagged" in step5c, (
        "deck-review.md step 5c must cite "
        "`TestF3_SingletonClassNeverFlagged` as the test pinning the "
        "D4 never-flag-a-singleton contract that #965 leaves intact."
    )
    assert "tautology" in step5c, (
        "deck-review.md step 5c must restate the D4 rationale "
        "('auto-shrink against itself is a tautology') so the reason "
        "the detector cannot be extended here travels with the doc."
    )


# --- deck-review.md — output surfaces ---------------------------------------


def test_review_summary_block_carries_the_field():
    """The `_summary.md` worked example must carry the new field.

    `deck-revise` step 7c reads it from there; a template that omits it
    produces reviews the reviser playbook cannot consume.
    """
    body = _read(DECK_REVIEW)
    idx = body.find('"auto_shrink": {')
    assert idx != -1, (
        "deck-review.md is missing the `_summary.md` `auto_shrink` "
        "block worked example."
    )
    block = body[idx : idx + 3000]
    assert "uncrosscheckable_singletons" in block, (
        "deck-review.md's `_summary.md` `auto_shrink` worked example "
        "must include the `uncrosscheckable_singletons` field."
    )
    assert '"source_side_rule"' in block, (
        "the `uncrosscheckable_singletons` worked example must show "
        "the per-entry shape (slide / class_name / source_side_rule) "
        "so the reviewer emits a parseable list."
    )


def test_review_findings_template_has_singleton_subsection():
    """`findings.md` must get a named, distinct subsection.

    Dropping these errors into the general lint list is the status quo
    the issue objects to: the reviser reads them as ordinary blocking
    errors with no sanctioned resolution.
    """
    body = _read(DECK_REVIEW)
    assert "## Singleton-class overflow errors" in body, (
        "deck-review.md's `findings.md` template must carry a "
        "`## Singleton-class overflow errors` subsection so these "
        "errors are visibly distinct from ordinary lint errors."
    )
    idx = body.find("## Singleton-class overflow errors")
    section = body[idx : idx + 4000]
    assert "deck-revise.md" in section, (
        "the `findings.md` singleton subsection must point at "
        "`deck-revise.md` step 7c — the reviser's playbook — so the "
        "resolution is reachable from where the error is reported."
    )
    # All three vision-sibling states must be spelled out; the
    # absent-sibling case is the one that must NOT license the hatch.
    for signal in ("refuted", "CONFIRMED", "not available"):
        assert signal in section, (
            f"the `findings.md` singleton subsection must cover the "
            f"'{signal}' vision-sibling outcome — a template that "
            "covers only the clean case invites the reviser to treat "
            "a missing sibling as a refutation."
        )


# --- deck-revise.md — the reviser playbook ----------------------------------


def test_revise_step7c_exists_and_reads_the_field():
    """Step 7c must exist and consume the review's machine-readable list."""
    body = _read(DECK_REVISE)
    idx = body.find("7c. **Resolve singleton-class")
    assert idx != -1, (
        "deck-revise.md is missing the `7c. **Resolve singleton-class "
        "`slide-content-overflow` errors via the vision cross-check**` "
        "step (issue #965)."
    )
    step7c = body[idx : idx + 9000]
    assert "uncrosscheckable_singletons" in step7c, (
        "deck-revise.md step 7c must read "
        "`lint.auto_shrink.uncrosscheckable_singletons` from the "
        "review sibling's `_summary.md`."
    )
    assert "#965" in step7c, (
        "deck-revise.md step 7c must cite issue #965."
    )


def test_revise_step7c_is_inactive_when_the_list_is_empty():
    """The common case must be an explicit no-op.

    Every other optional gate in this command documents its inactive
    path (parity lint, stale-token sweep); without it a reviser can
    invent work on threads with no singleton overflow errors.
    """
    body = _read(DECK_REVISE)
    idx = body.find("7c. **Resolve singleton-class")
    step7c = body[idx : idx + 9000]
    assert "inactive" in step7c, (
        "deck-revise.md step 7c must state that an empty / missing "
        "`uncrosscheckable_singletons` list makes the step inactive, "
        "so the common case is a documented no-op."
    )


def test_revise_step7c_puts_fixing_the_slide_first():
    """Fixing the slide must be the first resort, the hatch the last.

    An escape-hatch-first playbook converts a real overflow gate into a
    rubber stamp.
    """
    body = _read(DECK_REVISE)
    idx = body.find("7c. **Resolve singleton-class")
    step7c = body[idx : idx + 9000]
    signals = ("first resort", "FIRST resort", "default")
    found = [s for s in signals if s in step7c]
    assert found, (
        "deck-revise.md step 7c must frame fixing the slide as the "
        f"first resort. Expected one of {signals!r}."
    )
    assert "last" in step7c.lower(), (
        "deck-revise.md step 7c must frame the escape hatch as the "
        "last resort, not a peer option."
    )


def test_revise_step7c_requires_a_mandatory_vision_citation():
    """The escape-hatch branch must require a citable vision verdict.

    Without a mandatory citation the hatch is indistinguishable from
    silencing an unchecked error — which is the outcome the issue is
    trying to prevent, not create.
    """
    body = _read(DECK_REVISE)
    idx = body.find("7c. **Resolve singleton-class")
    step7c = body[idx : idx + 9000]
    assert "_review.json" in step7c, (
        "deck-revise.md step 7c must direct the reviser to the vision "
        "sibling's `_review.json` — the citable artifact, not a "
        "summary of it."
    )
    assert "vertical_overflow" in step7c, (
        "deck-revise.md step 7c must name the v1 `vertical_overflow` "
        "dimension the reviser reads."
    )
    assert "mandatory" in step7c.lower(), (
        "deck-revise.md step 7c must make the vision citation "
        "MANDATORY on the escape-hatch branch."
    )


def test_revise_step7c_covers_the_absent_vision_sibling():
    """A missing vision sibling must NOT license the escape hatch.

    This is the load-bearing negative case: the cheapest wrong reading
    of the new rule is "no vision finding on that slide, therefore
    clean" when the critic never ran.
    """
    body = _read(DECK_REVISE)
    idx = body.find("7c. **Resolve singleton-class")
    step7c = body[idx : idx + 9000]
    assert "No `<thread>.{N}.vision/` sibling exists" in step7c, (
        "deck-revise.md step 7c must handle the absent-vision-sibling "
        "case explicitly."
    )
    tail = step7c[step7c.find("No `<thread>.{N}.vision/` sibling exists") :]
    assert "NOT justified" in tail, (
        "deck-revise.md step 7c must state that the escape hatch is "
        "NOT justified when no vision sibling exists — silence from a "
        "critic that never ran is not a refutation."
    )


def test_revise_step7c_forbids_the_manual_pdf_look():
    """The reviser must not substitute their own read of the render."""
    body = _read(DECK_REVISE)
    idx = body.find("7c. **Resolve singleton-class")
    step7c = body[idx : idx + 9000]
    assert "Do NOT open the PDF yourself" in step7c, (
        "deck-revise.md step 7c must forbid the manual PDF look — the "
        "step exists precisely because `deck-review.md` step 5c says "
        "reviewers should not hand-confirm against the render."
    )


def test_revise_revision_log_has_the_resolutions_table():
    """Step 11's worked example must carry the audit-trail table."""
    body = _read(DECK_REVISE)
    assert "## Singleton-class overflow resolutions" in body, (
        "deck-revise.md step 11 (`_revision-log.md` worked example) "
        "must carry a `## Singleton-class overflow resolutions` "
        "subsection — the audit trail proving the hatch was earned."
    )
    idx = body.find("## Singleton-class overflow resolutions")
    section = body[idx : idx + 3000]
    assert "escape hatch —" in section, (
        "the `_revision-log.md` worked example must show an "
        "escape-hatch row so the citation format is concrete."
    )
    assert ".vision/_review.json" in section, (
        "the escape-hatch row in the `_revision-log.md` worked "
        "example must cite the vision sidecar path."
    )
    assert "fixed —" in section, (
        "the `_revision-log.md` worked example must also show a "
        "`fixed` row — fixing the slide is the expected default, so "
        "the canonical example must not be hatch-only."
    )


def test_revise_notes_section_carries_the_item():
    """§Notes for the reviser agent must carry the named contract item.

    Same shape as the restructure-authority item (#549): a named item
    the reviser-agent reads as priority guidance.
    """
    body = _read(DECK_REVISE)
    idx = body.find("## Notes for the reviser agent")
    assert idx != -1, (
        "deck-revise.md is missing the `## Notes for the reviser "
        "agent` section."
    )
    section = body[idx : idx + 8000]
    assert "Never silence a lint error you did not cross-check." in section, (
        "deck-revise.md §Notes for the reviser agent must carry the "
        "`Never silence a lint error you did not cross-check.` item "
        "(issue #965)."
    )
    item = section[
        section.find("Never silence a lint error you did not cross-check.") :
    ]
    assert "step 7c" in item, (
        "the note item must point at step 7c so the agent can reach "
        "the full playbook from the notes."
    )


# --- discoverability from both ends -----------------------------------------


def test_path_is_discoverable_from_both_commands():
    """Each command must point at the other.

    Issue #965's Option B test plan: the new section must be reachable
    from `deck-review.md` (where the error is raised) AND from
    `deck-revise.md` (where the reviser acts on it).
    """
    review = _read(DECK_REVIEW)
    revise = _read(DECK_REVISE)
    assert "deck-revise.md` step 7c" in review, (
        "deck-review.md must point forward at `deck-revise.md` step "
        "7c — the command that executes the resolution it names."
    )
    assert "deck-review.md`" in revise, (
        "deck-revise.md step 7c must point back at `deck-review.md` "
        "for the unified-gate contract that motivates the step."
    )


def test_deck_vision_carries_the_reciprocal_pointer():
    """`deck-vision.md` must know its v1 dim is load-bearing here.

    The critic is being promoted to the disambiguator of record for one
    slide class; scoring guidance that does not say so leaves the
    promotion invisible to the agent doing the scoring.
    """
    body = _read(DECK_VISION)
    assert "#965" in body, (
        "deck-vision.md must cite issue #965 where it explains that "
        "v1 `vertical_overflow` is the sanctioned cross-check for "
        "singleton-class overflow errors."
    )
    idx = body.find("#965")
    section = body[max(0, idx - 2000) : idx + 2000]
    assert "singleton" in section.lower(), (
        "deck-vision.md's #965 pointer must explain the singleton "
        "-class case that makes v1 load-bearing there."
    )


def test_marp_lint_escape_hatch_docstring_points_at_the_playbook():
    """The lint that owns the escape hatch must name its sanctioned use.

    `marp_lint.py`'s docstring documents the hatch; a reader who finds
    it there should learn when applying it is legitimate.
    """
    body = _read(MARP_LINT)
    idx = body.find("**Escape hatch.**")
    assert idx != -1, (
        "anvil/lib/marp_lint.py module docstring is missing the "
        "`**Escape hatch.**` section."
    )
    section = body[idx : idx + 1500]
    assert "#965" in section, (
        "marp_lint.py's escape-hatch docstring section must cite "
        "issue #965 — the decision that defines when applying the "
        "hatch is sanctioned."
    )
    assert "deck-revise.md" in section, (
        "marp_lint.py's escape-hatch docstring section must point at "
        "`deck-revise.md` step 7c for the sanctioned-use playbook."
    )
