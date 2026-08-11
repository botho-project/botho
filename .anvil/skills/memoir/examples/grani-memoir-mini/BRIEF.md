---
project: grani-memoir-mini
audience:
  - Family (primary)
hard_rules:
  - Every reconstructed quote and every checkable factual claim traces to a
    row in that chapter's provenance.md — cut the claim or mark it NOT_FOUND
    rather than invent a source.
corpus:
  - transcripts/
  - letters/
voice:
  subjects:
    - name: grani
      corpus: transcripts/**/*.md
documents:
  - slug: 00-introduction
    artifact_type: memoir
---

# Grani memoir (minimal synthetic example) — project brief

A **minimal synthetic worked example** (issue #740 v1 scope — NOT the full
`nitas-mama` dogfood; that vendored example is deferred to a follow-up
issue per SKILL.md §Scope guard). One chapter thread (`00-introduction`)
demonstrates the dual-corpus (#597) and subject-voice (#598) tiers
composed in a single `anvil:memoir` document, through a terminal
`AUDITED` version with a clean corpus-audit sibling.

## Reference material

- `transcripts/grani-01.md` — a short synthetic interview transcript (the
  `corpus:` tier's first root).
- `letters/1952-aug.md` — a short synthetic family letter (the `corpus:`
  tier's second root).

See `../expected-thread.N/README.md` for the structural contract this
example satisfies.
