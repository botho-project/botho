---
name: memo-comprehension
description: Cold-reader comprehension critic. Reads ONLY the memo body (blind — no BRIEF, no research, no rubric, no prior reviews), answers a fixed questionnaire in its own words, and diffs its answers against the intended message. Findings-only, non-gating.
---

# memo-comprehension — Cold-reader comprehension critic (blind-read sibling)

**Role**: comprehension critic (sibling, read-only).
**Reads**: during the **blind-read phase (Phase 1)**, ONLY the latest `<thread>.{N}/<thread>.md` body file and its `<thread>.{N}/exhibits/` directory — **nothing else**. No `BRIEF.md`, no `research/`, no `refs/`, no `rubric.md`, no `skeleton.md`, no prior `<thread>.{N}.review/` (or any other critic sibling). During the **diff phase (Phase 2)** the critic MAY additionally read `<project>/BRIEF.md` and `<thread>.{N}/skeleton.md` (when present) to establish intent. See §"Invocation contract: blindness is the instrument" below — the read restriction is the load-bearing mechanism of this critic.
**Writes**: `<thread>.{N}.comprehension/` (one sibling per reviewed version `N`).

This command is the **cold-reader comprehension layer** (issue #753). Every other phase in the memo lifecycle is an **atom-level or compliance-level check that the drafter can co-optimize**: the reviewer verifies each claim against its source, checks internal consistency, and scores rubric dimensions — all surface features a fluent drafter can satisfy while the document as a whole fails to *transmit*. The canary failure that motivated this critic: a memo scoring 43/44 (`advance: true`, 0 critical, dims 8 and 9 at 4/4, 22/22 refs verified) that the operator judged "almost word-soup bad ... mimicking the prosody of a report but not having a clear understanding of what we are trying to communicate." Every per-claim check passed because every claim *was* true and hedged; comprehension is a property of the whole, and the operator was the only judge in the loop measuring it.

The comprehension critic is the **only critic whose failure mode is structurally decorrelated from the drafter's**, because it must *produce* understanding rather than *assess* compliance. A rubric dimension cannot substitute for it: a dimension is scored by the reviewer, who reads the BRIEF and research first and therefore **cannot un-know** what the memo means (curse of knowledge, mechanized). Blindness is the whole instrument. It also cannot be Goodharted the way dims 8/9 were — the target is a second mind's reconstruction, not a checkable surface feature.

This is a **first-class critic sibling**, not a new framework: it plugs into the existing "N parallel critics, one reviser" primitive, consumes the canonical `_review.json` schema (`anvil/lib/review_schema.py`), and is discovered by `anvil/lib/critics.py::discover_critics` without any aggregator change. It is **findings-only** — the same posture as the optional `.audit/` / `.critic/` siblings: **no rubric dimension, no score, no gate, no critical flag.** Its `_review.json` carries all-`null` scores (it owns no rubric dimension) and an always-empty `critical_flags` list; its findings surface in `comments.md` at `major` severity and are consumed by `memo-revise` through the existing generic critic-sibling enumeration with zero code change (see §"Verdict pathway: findings-only, non-gating").

## Invocation contract: blindness is the instrument

**This is the one hard requirement of the command.** The comprehension critic MUST be dispatched as a **fresh agent that is told nothing about the project** — not the company, not the thesis, not the BRIEF, not the rubric, not the skeleton, not any prior review. It is handed **only the path(s) to the body file** (`<thread>.{N}/<thread>.md`) and its `exhibits/` directory, and asked to answer the Phase 1 questionnaire from those alone. This is not a suggestion or a nicety — a comprehension critic that has read the BRIEF cannot un-know the intended message, and its "reconstruction" is worthless because the curse of knowledge has already fired. The blind read is the measurement; everything else is scaffolding around it.

Operationally:

- The orchestrator dispatches Phase 1 to a **sub-agent with a body-only prompt**: "Here is a document. Read it and answer these seven questions in your own words. You have no other context." The sub-agent MUST NOT be handed, and MUST NOT go looking for, the BRIEF, `research/`, `refs/`, the rubric, the skeleton, or any critic sibling. The blind reader's answers (`answers.md`) are frozen before Phase 2 begins.
- Only **after** the blind answers are captured does the diff phase (Phase 2) read intent (`BRIEF.md` + `skeleton.md` when present). The diff is performed by the orchestrating pass (or a second agent), NOT by re-prompting the blind reader with the intent in hand — the blind reader's answers are an immutable Phase 1 artifact.
- If the same agent must perform both phases (no sub-agent dispatch available), it MUST write `answers.md` **completely** — every question answered from the body alone — **before** opening any intent file. The ordering is the contract: answers first, intent second, diff third. An `answers.md` written after the BRIEF was read is a contaminated measurement and MUST be discarded and re-run in a fresh session.

## The questionnaire (Phase 1 — blind)

The blind reader answers these seven questions **in its own words**, from the body + exhibits alone:

1. **What does this company sell, concretely?** (The product or service — not the mechanism, not the technology, not the vision. If you cannot name the thing a customer receives, say so.)
2. **Who buys it, and what do they pay?** (The customer and the transaction.)
3. **Why does this team win?** (The defensibility / edge, as the document states it.)
4. **What is the ask, and what does the money buy?** (The raise / decision requested and the use of proceeds.)
5. **What kills it?** (The dominant risk, as the document states it.)
6. **Every term you could not define from the document alone.** (A list of coinages, jargon, or variables the document uses as load-bearing without defining at first use.)
7. **What did the document not tell you that you needed to know?** (Gaps a serious reader would need filled to act on the document.)

Questions 1–5 are reconstruction questions (each gets a Phase 2 verdict). Question 6 is the **coined-jargon detector** — a blind reader listing terms it could not define IS the measurement (a deterministic n-gram-against-corpus version is brittle; the blind reader gives it for free). Question 7 is the **enrichment-candidate list**.

## Verdict vocabulary (Phase 2 — diff against intent)

With the BRIEF (and `skeleton.md` when present) establishing intent, each of the reconstruction answers (Q1–Q5) is classified with ONE of four verdicts:

- **`CLEAR`** — the blind reader recovered what was intended. The message transmitted. **No finding emitted.**
- **`GARBLED`** — an answer exists in the body, but the blind reader got it **wrong**. The claim is present but not transmitting (buried, hedged into vapor, fogged by mechanism language). **Finding emitted at `major`.**
- **`MISSING`** — the question is **unanswerable from the body**, AND the intent says it should be answerable. The document simply failed to say the thing it meant to say. **Finding emitted at `major`.**
- **`HONEST-GAP`** — the question is unanswerable because the answer is **genuinely unresolved**, AND the body **says so plainly**. **This is NOT a defect. No finding emitted.** This verdict is load-bearing for early-stage artifacts: a concept-stage memo legitimately cannot answer "what do we sell yet" — the critic's job is to verify the memo states that gap in plain words rather than fogging it with mechanism language. A memo that plainly says "the product does not yet exist; this raise funds the prototype" earns `HONEST-GAP` on Q1, not `MISSING`. The distinction between `MISSING` and `HONEST-GAP` is precisely whether the document is **honestly silent** (gap acknowledged in plain words → `HONEST-GAP`, non-defect) or **failing to communicate** (gap unacknowledged, or fogged behind mechanism language the reader mistook for an answer → `MISSING`, defect).

Severity ladder (verdict → `comments.md` severity):

| Verdict | Transmitted? | Finding | `comments.md` severity |
|---|---|---|---|
| `CLEAR` | yes | (none) | — |
| `GARBLED` | present but wrong | **yes** | `major` |
| `MISSING` | absent, should be present | **yes** | `major` |
| `HONEST-GAP` | absent, genuinely unresolved, **plainly stated** | (none) | — (explicit non-defect) |

Question 6 (undefined terms) becomes a **named jargon list** in `comments.md` — each term needs a definition at first use or deletion (emitted as `major` findings, one per load-bearing coinage, or grouped as one finding listing all terms). Question 7 (gaps) becomes **enrichment candidates** — advisory `scope: expand` suggestions, NOT defects (the reviser weighs them; they are not must-fix).

There is **no score, no rubric dimension, and no critical flag** anywhere in this ladder. `HONEST-GAP` in particular is the vocabulary's reason for existing: without it, a blind critic would penalize an early-stage artifact for honestly stating what it does not yet know, which would push drafters back toward the exact mechanism-fog this critic exists to catch.

## Outputs

```
<thread>.{N}.comprehension/
  _review.json         Canonical typed review payload per anvil/lib/review_schema.py (all scores null; empty critical_flags)
  answers.md           Phase 1: the blind reader's answers to the 7 questions, verbatim, frozen before Phase 2
  verdicts.md          Phase 2: per-question verdict (CLEAR / GARBLED / MISSING / HONEST-GAP) + diff-against-intent justification
  comments.md          Phase 3: GARBLED/MISSING findings + question-6 jargon list + question-7 enrichment candidates, severity-tagged
  _meta.json           { critic: comprehension, role, started, finished, model, scorecard_kind: human-verdict, rubric_id }
  _progress.json       Phase state (phase: comprehension; for_version: N)
```

**Atomicity** (issue #350, #376): the comprehension sibling dir is written **atomically** via the staged-sidecar primitive at `anvil/lib/sidecar.py`. The required files (`_review.json`, `answers.md`, `verdicts.md`, `comments.md`, `_meta.json`, `_progress.json`) are staged under a leading-dot sibling `.<thread>.{N}.comprehension.tmp/` during writing; on clean completion the staging dir is renamed (one atomic `Path.rename`) to the final `<thread>.{N}.comprehension/` name. A mid-cycle interrupt leaves a `.<thread>.{N}.comprehension.tmp/` dir on disk that the next invocation's `cleanup_one_staging(<thread>.{N}.comprehension)` per-critic sweep removes; the final-named dir never exists in partial form. Discovery (`anvil/lib/critics.py::discover_critics`) is unchanged — the leading-dot staging shape is invisible to the discovery glob.

### `_review.json` shape

The canonical payload conforms to `anvil/lib/review_schema.py::Review`:

- **`schema_version: "1"`** — pinned per the schema contract.
- **`kind: "judgment"`** — standard review kind (the comprehension critic does not use `tool_evidence` or `vision`).
- **`version_dir: "<thread>.{N}"`** — the version directory being reviewed (e.g., `"investment-memo.5"`).
- **`critic_id: "comprehension"`** — stable identifier; the trailing tag on the sibling dir name.
- **`model`** — model identifier that produced this review.
- **`rubric: "anvil-memo-v2"`** — echoes the memo rubric id (informational only — this critic owns no dimension).
- **`scores`** — the comprehension critic **owns no rubric dimension**, so it enumerates the full memo scorecard (dims 1–9) with **every `score` set to `null`**. The schema requires a full scorecard (`Review._validate_kind_required_fields` rejects an empty `scores` list for a scored review), so all nine dims are present; the aggregator's mean-of-non-null contract (`anvil/lib/critics.py::aggregate`) means these all-null entries **contribute nothing** to any dimension's merged score — the merged scores are determined entirely by the other critics (`.review/`, and any `.redteam/` / `.audit/`). This is the mechanism by which the comprehension critic is **additive and non-perturbing**: it can never move the total or the verdict.
- **`findings`** — one `Finding` per `GARBLED` or `MISSING` reconstruction verdict (Q1–Q5), plus jargon-list findings from Q6. Each finding carries:
  - `severity: "major"` — GARBLED/MISSING and jargon findings are all `major` (never `blocker` — this critic never gates).
  - `dimension: null` (or the nearest owning dim as a hint) — the finding is cross-cutting; it names no scored dimension because this critic owns none.
  - `evidence_span: "<thread>.md:L<start>-L<end>"` — pointer to the passage the blind reader misread (GARBLED) or the section where the answer should have been and was not (MISSING); for a jargon term, the line of first load-bearing use.
  - `rationale` — 1-2 sentences: which question, the verdict, and what the blind reader actually reconstructed vs. the intent.
  - `suggested_fix` — one sentence telling the reviser what to do: state the product in one plain sentence up front, define the coined term at first use or delete it, surface the buried answer, etc.
- **`critical_flags`** — **always the empty list `[]`.** The comprehension critic never emits a critical flag and never gates the state machine. This is the load-bearing distinction from `memo-redteam` (which emits `redteam_survives` / `redteam_unengaged` on load-bearing objections): comprehension is findings-only, same posture as `.audit/` / `.critic/`.
- **`total`** — `null` (no owned scores to sum).
- **`threshold`** — `35` (carried for completeness; the aggregator picks the first non-null threshold across critics, which will be the reviewer's).
- **`verdict`** — omitted (the aggregator computes verdict from the merged total + critical flags; this critic contributes neither).

### `answers.md` structure

The blind reader's Phase 1 output, frozen before any intent is read:

```markdown
# Blind-read answers — <thread>.{N}

> Written from the body + exhibits alone. No BRIEF, no research, no rubric, no skeleton, no prior review was read before these answers were frozen.

## Q1 — What does this company sell, concretely?
<answer in the reader's own words, or "I could not determine this from the document.">

## Q2 — Who buys it, and what do they pay?
<...>

## Q3 — Why does this team win?
<...>

## Q4 — What is the ask, and what does the money buy?
<...>

## Q5 — What kills it?
<...>

## Q6 — Terms I could not define from the document alone
- `<term>` — used at <line/section>; never defined at first use.
- ...

## Q7 — What the document did not tell me that I needed to know
- <gap>
- ...
```

### `verdicts.md` structure

The Phase 2 diff, one subsection per reconstruction question:

```markdown
# Comprehension verdicts — <thread>.{N}

## Q1 — What does this company sell? — **MISSING**
**Intent (from BRIEF/skeleton).** <one line: what the memo meant to convey.>
**Blind reader recovered.** <what the reader actually wrote for Q1.>
**Why MISSING.** <the answer is unanswerable from the body, and the intent says it should be answerable; the body never states the product in one place. Distinguish from HONEST-GAP: the body does NOT plainly acknowledge the gap — it fogs it with mechanism language.>

## Q3 — Why does this team win? — **CLEAR**
**Intent.** <...>
**Blind reader recovered.** <...>
**Verdict CLEAR.** The message transmitted; no finding.

## Q4 — What is the ask? — **HONEST-GAP**
**Intent.** <...>
**Blind reader recovered.** "I could not determine the product because the document says it does not yet exist."
**Verdict HONEST-GAP (non-defect).** The body plainly states the gap ("it does not yet exist") rather than fogging it. No finding emitted. This verdict gives an early-stage artifact credit for stating a gap in plain words.
```

### `_meta.json`

```json
{
  "critic": "comprehension",
  "role": "memo-comprehension.md",
  "started": "<ISO-8601 UTC>",
  "finished": "<ISO-8601 UTC>",
  "model": "<model id, e.g., claude-opus-4-8>",
  "scorecard_kind": "human-verdict",
  "rubric_id": "anvil-memo-v2",
  "rubric_total": 44,
  "advance_threshold": 35,
  "verdicts": {
    "CLEAR": <count>,
    "GARBLED": <count>,
    "MISSING": <count>,
    "HONEST-GAP": <count>
  },
  "undefined_terms": <N>,
  "enrichment_candidates": <N>
}
```

`scorecard_kind: human-verdict` per `anvil/lib/snippets/scorecard_kind.md`: the comprehension critic's prose (`answers.md` + `verdicts.md`) is read narratively by the reviser; the `_review.json` carries the structured findings payload; there is no machine-summary scorecard because this critic scores no dimension.

## Procedure

1. **Discover state**: identify the latest version directory `<thread>.{N}/` (the highest `N` carrying a `<thread>.md` body file). The comprehension critic always runs against the latest version — it does NOT process older versions retroactively. Then **sweep a stale staging dir from a prior interrupt of THIS critic on THIS version** by invoking `anvil/lib/sidecar.py::cleanup_one_staging(<thread>.{N}.comprehension)` (the per-critic, parallel-safe sweep — issue #376). This removes ONLY a leftover `.<thread>.{N}.comprehension.tmp/` from a previously-killed run of this same critic on THIS version.

2. **Resume check**: per the staged-sidecar shape (issue #350), a completed comprehension sibling means the final-named `<thread>.{N}.comprehension/` dir exists — the atomic-rename contract guarantees the dir only exists when complete. If `<thread>.{N}.comprehension/` exists, exit early — the sibling is complete (idempotent). The completed sibling is read-only; re-run only by creating a NEW sibling at the next version.

3. **Open the staged sidecar** for the comprehension dir by invoking the context manager `anvil/lib/sidecar.py::staged_sidecar(final_dir=<thread>.{N}.comprehension, required_files=["_review.json", "answers.md", "verdicts.md", "comments.md", "_meta.json", "_progress.json"])`. Every file write from this step through step 9 MUST land **inside the yielded staging directory** (the path of the shape `.<thread>.{N}.comprehension.tmp/`), NOT inside the final `<thread>.{N}.comprehension/` path. On clean context exit, the staged sidecar primitive verifies every name in the manifest exists, then atomically renames the staging dir to its final name. Then, **inside the staging dir**, initialize `_progress.json`: `phases.comprehension.state = in_progress`, `phases.comprehension.started = <ISO>`, `for_version = N`.

   **Non-Python-driver ordering (fail-open, manual fallback)** — issue #645: `staged_sidecar` is a Python context manager a manual/agent session cannot hold open across discrete file-writing tool calls. Use the equivalent CLI shim instead of writing straight into the final dir (which reopens the #350 partial-write defect). In an installed consumer repo (anvil vendored under `.anvil/`), prefix each invocation with `uv run --project .anvil`; in the anvil source repo the bare form works:
   - `python -m anvil.lib.sidecar stage <thread>.{N}.comprehension` → prints the staging path (`.<thread>.{N}.comprehension.tmp/`); refuses with a nonzero exit if the final dir already exists.
   - Write **all** required files into that printed staging path — never into the final `<thread>.{N}.comprehension/` name.
   - `python -m anvil.lib.sidecar commit <thread>.{N}.comprehension --required _review.json,answers.md,verdicts.md,comments.md,_meta.json,_progress.json` → verifies the manifest, then atomically renames staging → final. Nonzero exit (1) leaves the staging dir in place with no partial final dir if any file is missing.
   - The stale-staging sweep of step 1 has an exact CLI analog: `python -m anvil.lib.sidecar cleanup <thread>.{N}.comprehension`.

   When even `python`/`uv` is unavailable, reproduce the staging contract by hand: sweep any leftover `rm -rf .<thread>.{N}.comprehension.tmp/`, `mkdir` the `.tmp/` dir, write every required file into it (writing `_progress.json` LAST), confirm all six files are present, **then** `mv .<thread>.{N}.comprehension.tmp <thread>.{N}.comprehension` as the last step (POSIX same-filesystem dir rename is atomic). Stamp `_meta.json` with `"atomicity_fallback": "manual-mv"` so a reader can tell atomicity was reproduced by hand. (If your agent harness pattern-matches and rejects the `comments.md` filename on a `Write`, a Bash-heredoc write into the staging dir is an accepted fallback — see `anvil/lib/snippets/critics.md` §"Orchestrator output-file guard collisions".)

4. **Phase 1 — blind read.** Dispatch the blind read per §"Invocation contract: blindness is the instrument". The blind reader reads ONLY `<thread>.{N}/<thread>.md` and `<thread>.{N}/exhibits/` and answers the seven questions from §"The questionnaire" in its own words. Write the frozen answers to `answers.md` (staging dir) per the §"`answers.md` structure" shape. **Do NOT read any intent file (BRIEF, skeleton, research, refs, rubric, or any critic sibling) before `answers.md` is complete** — the ordering is the contract.

5. **Phase 2 — diff against intent.** NOW (and only now) read `<project>/BRIEF.md` and `<thread>.{N}/skeleton.md` (when present) to establish the intended message. For each reconstruction question (Q1–Q5), diff the blind reader's answer against intent and classify with ONE of `CLEAR` / `GARBLED` / `MISSING` / `HONEST-GAP` per §"Verdict vocabulary". Be careful on the `MISSING` vs. `HONEST-GAP` boundary: `HONEST-GAP` requires the body to **plainly acknowledge** the gap in the reader's own words; a gap fogged behind mechanism language the reader mistook for an answer is `MISSING` (or `GARBLED`), never `HONEST-GAP`. Write `verdicts.md` (staging dir) per the §"`verdicts.md` structure" shape.

6. **Phase 3 — findings.** Assemble `comments.md` (staging dir):
   - Each `GARBLED` and each `MISSING` verdict → one `comments.md` entry at `**major**` severity, keyed to a section heading, carrying the evidence span and a one-sentence suggested fix.
   - Each `HONEST-GAP` verdict → **no finding** (explicit non-defect). Optionally note it in a `## Non-defects (HONEST-GAP)` audit subsection so the reviser sees it was considered and deliberately not flagged.
   - Question-6 undefined terms → a `## Undefined terms (define at first use or delete)` subsection listing each coinage at `**major**`; the reviser must define each load-bearing term at first use or delete it.
   - Question-7 gaps → a `## Enrichment candidates (scope: expand — advisory)` subsection; these are advisory, NOT must-fix.

7. **Write `_review.json`** (staging dir): assemble the canonical typed payload per §"`_review.json` shape". Enumerate all nine dims with `score: null`, populate `findings` (GARBLED/MISSING/jargon), set `critical_flags: []`. Validate by constructing the `Review` object (`Review.model_validate(...)`) before writing — a `pydantic.ValidationError` indicates malformed output that must be corrected.

8. **Write `_meta.json`** (staging dir) per §"`_meta.json`".

9. **Update `_progress.json`** (staging dir): `phases.comprehension.state = done`, `phases.comprehension.completed = <ISO>`. This is the LAST file write before the context manager exits — the manifest verification + atomic rename at exit requires `_progress.json` to be present. Then **exit the `staged_sidecar` context block**: the primitive verifies every name in the required-files manifest exists, then atomically renames `.<thread>.{N}.comprehension.tmp/` → `<thread>.{N}.comprehension/`.

10. **Report**: print the path to the (now-renamed) comprehension sibling and a one-line status (e.g., `Comprehension investment-memo.5.comprehension/ (Q1 MISSING, Q2 CLEAR, Q3 GARBLED, Q4 HONEST-GAP, Q5 CLEAR; 3 undefined terms, 2 enrichment candidates → 3 major findings)`).

## Verdict pathway: findings-only, non-gating

The comprehension critic does NOT compute `advance` and does NOT gate. It writes findings into `_review.json` and `comments.md`; the existing memo pipeline does the rest:

- `anvil/lib/critics.py::discover_critics(<thread>.{N})` finds the `<thread>.{N}.comprehension/` sibling alongside `<thread>.{N}.review/` and any other critic siblings.
- `anvil/lib/critics.py::aggregate([...])` merges the reviews. The comprehension critic's all-`null` scores contribute nothing to any dimension's mean-of-non-null, so the merged total and per-dim scores are **identical** whether or not the comprehension sibling is present. Its `critical_flags: []` adds nothing to the union. The comprehension critic is therefore **provably non-perturbing** to the verdict.
- The reviser at `commands/memo-revise.md` step 6 reads **ALL** `<thread>.{N}.<critic>/` siblings generically (`.review/`, `.audit/`, `.critic/`, and now `.comprehension/`) — see `commands/memo-revise.md` line "Every other `<thread>.{N}.<critic>/` sibling discovered on disk". The comprehension critic's `comments.md` `major` findings flow into the reviser's revision plan through the **existing** severity-tagged `comments.md` enumeration with **zero code or doc change** to `memo-revise`. Under the default `--scope important`, the comprehension critic's `major` findings ARE addressed.

**No aggregator change is needed. No `memo-revise.md` change is needed.** The comprehension critic is a new critic *tag*, not a new framework: `discover_critics` already finds `<thread>.{N}.<tag>/` siblings; `aggregate` already handles all-null scorecards via mean-of-non-null; `memo-revise` already consumes every critic sibling's `comments.md` generically.

## Non-gating

**Absence of a comprehension sibling does NOT block the state machine.** A memo thread with no `<thread>.{N}.comprehension/` proceeds normally through `draft → review → revise → figures` per `SKILL.md` §"State machine". The comprehension critic is opt-in input, not required output. The orchestrator MAY recommend running `memo-comprehension` as an optional parallel critic alongside `memo-review`, but does NOT enforce it. This is the same property that lets every other opt-in critic ship incrementally: existing memo threads have no comprehension sibling and continue to advance unchanged.

## Idempotence and resumability

- A completed comprehension sibling (`phases.comprehension.state == done` AND `_review.json` exists) is never re-run automatically; the final-named dir is immutable.
- A crashed comprehension run (mid-cycle interrupt) manifests as a leading-dot `.<thread>.{N}.comprehension.tmp/` directory; the next invocation's `cleanup_one_staging` sweep removes it and re-runs from scratch.
- Validation is by file existence (does `_review.json` parse via `Review.model_validate`? do `answers.md` / `verdicts.md` exist?), not solely by the progress flag.

## Portability note (future work — out of scope here)

The blind-read questionnaire is memo-shaped (product / customer / edge / ask / risk). The critic pattern is **portable to `deck` / `proposal` / `report`** with the questionnaire swapped per artifact type — but that cross-skill rollout is deliberately OUT of scope for this issue (issue #753 ships memo-only, matching how `memo-redteam` shipped memo-only before any portability discussion). A follow-up issue owns the per-artifact-type questionnaire swaps once the memo shape is validated by the canary.

## Notes for the comprehension agent

- **Blindness is the whole point.** Do NOT read the BRIEF, research, refs, rubric, skeleton, or any prior review before `answers.md` is frozen. If you already know what the memo means, you are the wrong instrument.
- **Answer in your own words.** Do not quote the document's phrasing back at it — a reconstruction in your own words is what reveals whether the message transmitted. If you find yourself reaching for the document's coinages to answer, that is itself a Q6 signal.
- **`HONEST-GAP` is a gift, not a defect.** An early-stage memo that plainly says "we don't have a product yet, this raise funds the prototype" is doing the honest thing. Give it `HONEST-GAP` on Q1 and emit no finding. Reserve `MISSING` for documents that failed to say what they meant to say; reserve `GARBLED` for documents whose answer is present but fogged.
- **Be concrete on the jargon list.** A term earns a Q6 entry when it is **load-bearing** (the argument leans on it) AND **undefined at first use**. A defined-once-then-reused variable that you could track is fine; a coinage used as a protagonist without ever being paid for is exactly the finding.
- **Never gate.** You emit no critical flag, no score, no verdict. Your `_review.json` `critical_flags` is always `[]` and your `scores` are always all-`null`. Your job is to surface findings for the reviser, not to block the pipeline.

**Snippet references**: See `anvil/lib/snippets/scorecard_kind.md` for the `human-verdict` scorecard kind. See `anvil/lib/snippets/progress.md` for the `_progress.json` read-merge-write recipe and `anvil/lib/snippets/timestamp.md` for the ISO-8601 UTC timestamp convention. See `anvil/skills/memo/commands/memo-redteam.md` for the sibling critic-command precedent this command mirrors (new critic sibling, read-only, opt-in, `human-verdict` scorecard, atomic sidecar via `staged_sidecar`) — noting that memo-redteam owns dims 2/3 and emits critical flags, whereas memo-comprehension owns no dimension and never gates.

## Git sync (opt-in, off by default)

Per `anvil/lib/snippets/git_sync.md`: if `.anvil/config.json` exists and `git.commit_per_phase` is `true`, end this phase: stage only the dirs this phase wrote, commit as `anvil(memo/comprehension): <thread>.{N} [<state>]`, push if `git.push` is `true`. Git failures warn and continue — never fail the phase. When the config or knob is absent, skip this step entirely (default off).

This phase's specifics:

- **Ordering**: after the staged-sidecar atomic rename (issue #350) lands the final-named `<thread>.{N}.comprehension/` — so only complete sidecars are ever committed.
- **Staging target**: ONLY this command's own comprehension sidecar (never sibling critics' dirs — the narrow scope keeps the hook safe under parallel critic fan-out).
- **Commit**: `anvil(memo/comprehension): <thread>.{N} [<state>]` — the bracket carries the thread's current derived state per SKILL.md §State machine, since comprehension is non-gating and does not advance the state machine.
