# Findings — 00-introduction.1 (exhaustive corpus-provenance audit, kind: tool_evidence)

## Anchor-resolution pre-pass (§Section 4a, issue #868)

`python -m anvil.lib.provenance_anchor check provenance.md
transcripts/ letters/` ran before classification (deterministic, exit 0
regardless of outcome). Result: 4 rows `RESOLVED` (anchor text found
intact at the cited `Line range`), 1 row `NO_ANCHOR` (the `NOT_FOUND`
row below has no anchor to check — nothing to anchor when no corpus
passage supports the claim). `anchor_drift_count: 0` — nothing has
moved in this worked example's corpus since this version was drafted,
so classification below proceeds against each row's cited `Line range`
exactly as it would pre-#868.

| Claim | Source file | Line range | Classification | tool_calls evidence |
|-------|-------------|------------|-----------------|----------------------|
| "Well, I remember it clear as anything." | transcripts/grani-01.md | 5 | VERIFIED | Read transcripts/grani-01.md:5 — exact match: "Well, I remember it clear as anything." |
| "It was a Tuesday, hot as blazes" | transcripts/grani-01.md | 5-6 | VERIFIED | Read transcripts/grani-01.md:5-6 — exact match: "It was a Tuesday, hot as blazes". |
| Grani was eight years old the day the factory burned | transcripts/grani-01.md | 11 | PARAPHRASE_OK | Read transcripts/grani-01.md:11 — "I was eight years old" supports the paraphrase; substance present, wording is authorial paraphrase. |
| The journey west took six weeks | letters/1952-aug.md | 3-4 | PARAPHRASE_OK | Read letters/1952-aug.md:3-4 — "It has been six weeks since we left Carthage" supports the paraphrase. |
| Ruth reached Tulsa by the end of the month | letters/1952-aug.md | 5 | NOT_FOUND | Read letters/1952-aug.md:5 — "We should reach Tulsa by the end of the month" states intent, not a confirmed arrival; no passage in either corpus root confirms arrival. Not a critical flag: the chapter prose already reflects this uncertainty explicitly. |

Every `MISMATCH`/`NOT_FOUND`/`FABRICATED` row above carries a non-empty
`tool_calls` evidence entry per `anvil/lib/snippets/provenance.md`
§Section 4 rule 4 (only the NOT_FOUND row applies here). No `DRIFTED`
row exists in this pass, so no anchor-drift finding is emitted (see
`provenance_summary` in `_progress.json`: `anchor_drift_count: 0`).
