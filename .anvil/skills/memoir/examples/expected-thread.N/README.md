# Expected thread.N — illustrative snapshot

This directory documents the **structural contract** that the vendored
minimal synthetic example (`../grani-memoir-mini/`) satisfies — which
files exist, which fields parse, which rubric stamps land — NOT the
exact prose. This is an **illustrative reference**, not a strict golden
file (the same non-golden-file posture `essay`/`primer`/`spec` use for
their examples).

## What this example is (and is not)

Per issue #740's own "Scope guard — v1 / deferred" section, v1 ships a
**minimal synthetic worked example** — one project, one chapter thread,
one terminal `AUDITED` version — sufficient to exercise the skeleton
tests. It is explicitly **NOT** a full `nitas-mama` dogfood (the `spec`
Phase 4 / #709 precedent): that vendored example is a deferred follow-up
issue once this skeleton lands, the same sequencing `primer` and `spec`
both used.

## Structure

```
grani-memoir-mini/                          project root
  BRIEF.md                  Frontmatter: project + corpus: [transcripts/,
                            letters/] + voice: {subjects: [{name: grani, ...}]}
                            + documents: [{slug: 00-introduction,
                            artifact_type: memoir}]
  transcripts/grani-01.md   Synthetic corpus root 1 (the subject's own
                            spoken corpus, doubling as the voice.subjects
                            corpus)
  letters/1952-aug.md       Synthetic corpus root 2
  00-introduction/                          thread dir (named for the slug)
    00-introduction.1/                      terminal AUDITED version
      00-introduction.tex   LaTeX body; slug-echo filename
      provenance.md         5-row claim -> source map (2 VERIFIED-shaped,
                            2 PARAPHRASE_OK-shaped, 1 explicit NOT_FOUND)
      _progress.json        { version: 1, phases.draft.state: "done",
                              metadata.corpus_dirs_resolved,
                              metadata.subject_voice_exemplars,
                              metadata.max_iterations: 4,
                              metadata.iteration_cap_rationale: null }
                            (the cap audit trail — SKILL.md §"Iteration
                            cap and override contract"; a revised version
                            dir additionally carries
                            metadata.revision_class)
    00-introduction.1.review/               reviewer sibling (rubric /44)
      verdict.md             Total 41/44; advance: true; critical flags: none
      scoring.md              9-row table (# | Dimension | Weight | Score | Justification)
      comments.md            Line-level comments
      _summary.md            Machine-readable summary (rubric, provenance_back_check,
                            subject_voice_grounding, scope_distribution)
      _meta.json             scorecard_kind: "human-verdict"; rubric_id: "anvil-memoir-v1";
                            rubric_total: 44; advance_threshold: 39  (the #346 stamps)
      _progress.json
    00-introduction.1.audit/                general auditor sibling (ALWAYS written)
      verdict.md             audit_clean: true; critical flags: none
      findings.md            Per-claim factual/narrative-consistency table
      comments.md
      _summary.md
      _meta.json             Same #346 stamps as review
      _progress.json
    00-introduction.1.corpus-audit/         exhaustive corpus-audit sibling
                                            (ONLY because corpus: is active)
      verdict.md             audit_clean: true; 5 claims checked, 0 fabrication-class flags
      findings.md            kind: tool_evidence — per-provenance-row five-way
                            classification with tool_calls evidence
      comments.md
      _summary.md            provenance_summary: {total_claims: 5, verified: 2,
                            paraphrase_ok: 2, mismatch: 0, not_found: 1, fabricated: 0}
      _meta.json             Same #346 stamps
      _progress.json
```

## Why `.corpus-audit/` exists here (and would NOT exist without `corpus:`)

The vendored `BRIEF.md` declares a top-level `corpus:` list, so
`memoir-audit` writes BOTH the general `.audit/` sibling (always) AND the
exhaustive `.corpus-audit/` sibling (conditional). If `corpus:` were
absent, only `.audit/` would exist, carrying a `major` finding
recommending the operator declare `corpus:` — SKILL.md §Dual-corpus
provenance and `rubric.md` §Dimension 1 both document this
byte-identical-when-absent posture. The shipped
`tests/test_memoir_example_brief_parses.py` pins the presence of all
three siblings for this example specifically because `corpus:` is
declared.

## The dual-tier composition (the load-bearing feature)

This is a small example, but it is genuinely a **composition** of both
primitives at once, not a demonstration of either alone:

- **Dual-corpus provenance (#597)**: `corpus:` declares TWO roots
  (`transcripts/`, `letters/`); `provenance.md` cites both.
- **Subject voice tier (#598)**: `voice.subjects` declares Grani, whose
  transcript corpus doubles as both the #597 factual ground truth and
  the #598 spoken-cadence ground truth for the one quoted dialogue line
  — the same corpus file serving two independent contracts
  simultaneously, exactly as SKILL.md §Scope boundary documents.

## The #346 rubric stamps

All three critic siblings' `_meta.json` carry the per-review version
stamps: `scorecard_kind: "human-verdict"`, `rubric_id:
"anvil-memoir-v1"`, `rubric_total: 44`, and the **audit-grade**
`advance_threshold: 39` (memoir is a /44 rubric with sourcing fidelity
as the dominant dim 1, scored on the ≥39 audit-grade band per SKILL.md
§Advance threshold rationale — not the general ≥35 band `primer` uses).

## Why not a full text snapshot

- This is a synthetic example authored specifically to exercise the
  skeleton tests — it is not trimmed from a real production thread the
  way `primer`'s and `spec`'s vendored examples are.
- A realized memoir chapter's prose, dimension scores, and per-claim
  findings will vary across real runs and models exactly as every other
  vendored anvil example documents.
