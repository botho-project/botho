---
name: primer
description: Draft, review, audit, revise, and illustrate long-form pedagogical explainers (teach-from-intuition companions to a formal spec) through the canonical anvil lifecycle. Markdown source-of-truth with an optional PDF render; ends at AUDITED with a documented publish handoff. Pedagogical scaffolding is the owned dominant rubric dimension.
domain: primer
type: skill
user-invocable: false
---

# anvil:primer — Long-form pedagogical explainers (teach-from-intuition companion to a formal spec)

The `primer` skill produces **long-form pedagogical explainers** (markdown body, multi-section, typically 10–30× an essay's length) through the report-shaped anvil lifecycle: `draft → review + audit (parallel) → revise → … → READY/AUDITED → figures`. The canonical model is **"Mechanics of MobileCoin"**: a ground-up teaching companion that sits *alongside* a formal whitepaper, teaching the same primitives from intuition rather than restating them in notation.

What makes the class distinct is **pedagogy**: the artifact succeeds or fails on whether the reader *learns the subject*, not on whether it sounds like its author (that is `essay`) and not on spec-completeness or evidentiary rigor (that is `report`). A teaching text deliberately *defers* rigor to the spec ("for the formal treatment, see §X") and spends its ink on intuition, analogy, and worked examples — the opposite tilt from `report`. The rubric is therefore weighted so that **dim 1 (Pedagogical scaffolding / learnability) dominates** (weight 7), the way `essay` tilts toward voice and `memo` toward substance.

## Relationship to `anvil:report`

`primer` borrows `report`'s **lifecycle shape** (draft → parallel review+audit → revise → AUDITED, plus a figures phase and a markdown-source-of-truth + optional-PDF output) as its closest precedent. It is **a new skill, not a `report` parameterization** — per the CLAUDE.md convention "**skill identity = artifact identity**" (Anvil ships one skill per standardized artifact type, not parameterized meta-skills with a `--type` flag). Where `report` and `primer` share infrastructure (the render pipeline, the sidecar primitive, the rubric-stamping contract) that sharing lives in `anvil/lib/`, not in a unified skill.

| | `anvil:report` | `anvil:essay` | **`anvil:primer` (this skill)** |
|---|---|---|---|
| Length | medium formal | 500–1500 words | long-form, multi-section |
| Register | formal / customer-grade | personal voice | **teaching / intuition-first** |
| Dominant rubric dim | substance + evidence | Voice fidelity (w7) | **Pedagogical scaffolding / learnability (w7)** |
| Advance threshold | ≥39/44 (customer-facing) | ≥35/44 (general) | **≥35/44 (general — educational collateral)** |
| Audit phase | factual / evidence audit | none | **factual audit + spec-consistency audit** |
| Output | markdown + PDF | markdown only | **markdown source + optional PDF** |
| Relationship to a spec | standalone | standalone | **explicitly derivative: cross-reference, never duplicate or contradict** |

A `primer` `documents:` entry declares `artifact_type: primer`; memo commands fail loudly when pointed at it (it selects no memo rubric overlay — it is a skill-identity artifact type, not a memo subtype).

## Artifact contract

A **primer thread** is a single long-form explainer authored across one or more revisions, identified by a slug (e.g., `botho-from-the-basics`, `mechanics-of-x`). Each thread lives inside a **project root** carrying a project-level `BRIEF.md` (the post-#295/#296 canonical model); the body markdown inside each version directory **echoes the slug** (`<slug>.md`):

```
<project>/                     Project root
  BRIEF.md                     Project-level brief (frontmatter `documents:` list +
                               optional per-doc `spec_ref` key — see §Spec-ref contract)
  research/                    Optional shared evidence pool
  <thread>/                    Thread directory (named for the slug)
    refs/                      Optional reference material (sources, transcripts)
    <thread>.1/                First drafted version (immutable once written)
      <thread>.md              Primer body (filename echoes the slug per #295)
      exhibits/                Figures produced by primer-figures (mmdc → PNG, etc.)
      <thread>.pdf             Optional PDF render (produced by primer-figures)
      _progress.json           Phase state for this version
      changelog.md             (revisions only) Maps prior critic notes to changes
    <thread>.1.review/         Reviewer sibling (read-only once written)
      verdict.md               Advance / block + total /44 + critical flags
      scoring.md               Per-dimension scores against rubric.md
      comments.md              Line-level comments keyed to the body markdown
      _summary.md              Machine-readable summary blocks (spec_ref, gates, …)
      _meta.json               human-verdict scorecard kind + #346 rubric stamps
      _progress.json           Phase state for the reviewer
    <thread>.1.audit/          Auditor sibling (read-only once written)
      verdict.md               Audit verdict + critical audit flags
      findings.md              Per-claim factual + spec-consistency findings
      comments.md              Line-level audit comments
      _summary.md              Machine-readable audit summary (spec_ref resolution)
      _meta.json               human-verdict scorecard kind + #346 rubric stamps
      _progress.json           Phase state for the auditor
    <thread>.2/                Revised version (consumes v1 + BOTH critic siblings)
    ...
    <thread>.{N}/              Terminal version, marked AUDITED in its _progress.json
```

Versioned dirs (`<thread>.{N}/`) and critic sibling dirs (`<thread>.{N}.<critic>/`) are **immutable once their `_progress.json` records the phase as `done`**. Revisions are produced as a new version dir, never by editing in place. The review and audit siblings consume the same `<thread>.{N}/` and write to disjoint paths — they are **pure parallel critics** in the "N parallel critics, one reviser" sense (the `report` precedent).

## Spec-ref contract (optional companion input)

The defining constraint of a *companion* is: **teach then point ("see §X of the spec"); do not duplicate the spec's formal sections and do not contradict them.** To make that constraint audit-checkable, a primer thread may declare an optional `spec_ref` in its `BRIEF.md` `documents:` entry — a freeform path or glob naming the formal sibling artifact this primer teaches alongside:

```yaml
documents:
  - slug: botho-from-the-basics
    artifact_type: primer
    spec_ref: ../whitepaper/whitepaper.5/whitepaper.md
```

The path is resolved **project-root first, then consumer-root**, the same walk `report`'s `prior_reports[]` paths and the `voice:` docs use, via `anvil/lib/project_brief.py::resolve_spec_ref(project_dir, slug)` (never raises on absence; a declared-but-missing path comes back as a structured `missing: true` entry). The activation contract follows the framework-wide #428/#449 posture exactly (`report`'s `customer:` key, `essay`'s `voice:` block):

- **`spec_ref` declared and resolves** → the **spec-consistency tier is ACTIVE**. `primer-audit` reads the resolved spec document and performs the spec-consistency sweep: any primer claim that *contradicts* the cited spec is the audit-side critical flag **"Contradicts cited spec"**; any formal section the primer *duplicates* instead of cross-referencing is surfaced by `primer-review` as the review-side critical flag **"Duplicates formal spec section"**. Dim 5 (*Spec cross-reference discipline*) scores against the resolved spec.
- **`spec_ref` absent (undeclared)** → the tier is **inactive / silent-off**. Dim 5 scores on the primer alone (no spec cross-check possible — the two spec-consistency critical flags cannot fire), and both `primer-review` and `primer-audit` record a **`major` finding recommending the operator declare `spec_ref`** (a companion with no declared spec cannot have its defining constraint mechanically enforced — a defect to surface, never a crash, never a false critical flag). This is the standard "declared-but-missing is a defect to surface; absent-and-undeclared is silent/off" activation contract.
- **`spec_ref` declared but unresolvable (bad path / empty glob)** → the tier **ACTIVATES**; `resolve_spec_ref` returns a `missing: true` entry (never raises), and the breakage surfaces as a **`major` finding** directing the operator to fix the path. The audit proceeds without the cross-check (graceful degradation — the same `report` customer-context / `essay` voice-docs posture); no critical flag fires from an unresolvable spec.

## State machine

Per-thread state, derived from on-disk evidence (not flags), following the `report` parallel-critic shape (the two-stage `CUSTOMER-READY` promotion is deliberately NOT adopted — a primer is educational collateral, not a customer-liability deliverable, so `AUDITED` is the terminal state):

```
EMPTY → DRAFTED → REVIEWED+AUDITED → REVISED → … → READY → AUDITED
```

| State | Evidence |
|---|---|
| `EMPTY` | No `<thread>.{N}/` directories exist |
| `DRAFTED` | Latest `<thread>.{N}/` exists with `<thread>.md` (slug-echo per #295) and `_progress.json.draft == done`; no critic siblings at the same `N` |
| `REVIEWED-PARTIAL` | `<thread>.{N}.review/verdict.md` exists for the latest `N` (without `.audit/`) — transient; not advance-eligible |
| `AUDITED-PARTIAL` | `<thread>.{N}.audit/verdict.md` exists for the latest `N` (without `.review/`) — transient; not advance-eligible |
| `REVIEWED+AUDITED` | BOTH `<thread>.{N}.review/verdict.md` AND `<thread>.{N}.audit/verdict.md` exist for the latest `N` |
| `REVISED` | A `<thread>.{N+1}/` exists after a prior `REVIEWED+AUDITED` state at `N` |
| `READY` | Latest `REVIEWED+AUDITED` records BOTH `advance: true` (review total ≥35/44) AND a clean audit (no unresolved audit critical flag) |
| `AUDITED` | Same as `READY` for this skill — the standard anvil terminal state, reached once both critic siblings clear. `primer-figures` may then produce the optional `<thread>.pdf` |

Thresholds: **≥35/44 advances** (general tier — educational collateral, NOT the customer-facing ≥39 band used by `report`/`ip-uspto`/`datasheet`). Any critical flag (review-side or audit-side) short-circuits regardless of total — block until addressed. Iteration cap: default `max_iterations: 4`; consumer overrides via the project-BRIEF paired override (`max_iterations` + `iteration_cap_rationale`, the #349 memo contract). Exceeding the cap marks the thread `BLOCKED` (human review).

**Why "REVIEWED+AUDITED" rather than running them serially?** Both siblings consume the same `<thread>.{N}/` and write to disjoint paths — pure parallel critics. The reviewer scores pedagogy/prose/audience (dims 1–3, 5–9 judgment side); the auditor verifies factual correctness (dim 4 audit twin) and spec-consistency (dim 5 audit twin). Sequential execution buys nothing here; the reviser consumes both.

## Publish handoff contract

**The skill ends at `AUDITED`.** Publishing — the "Mechanics of MobileCoin" precedent is web HTML — stays **consumer-native**, exactly as `anvil:report`'s CUSTOMER-READY precedent keeps customer delivery outside the framework and `anvil:essay`'s publish handoff keeps site deploys native. What the handoff guarantees to the consumer's publish tooling:

1. **A `.latest`-resolvable body**: `<thread>/<thread>.{N}/<thread>.md` for the highest `N`; resolution semantics per `anvil/lib/latest_resolution.py::resolve_latest`.
2. **An AUDITED version**: `<thread>.{N}.review/verdict.md` records `advance: true`, total ≥35/44, zero unresolved review critical flags; `<thread>.{N}.audit/verdict.md` records a clean audit (no unresolved factual or spec-consistency critical flag).
3. **Stamped critic metadata**: both siblings' `_meta.json` carry `scorecard_kind: "human-verdict"` plus the #346 stamps (`rubric_id: "anvil-primer-v1"`, `rubric_total: 44`, `advance_threshold: 35`).
4. **An optional PDF**: when `primer-figures` has run, `<thread>.{N}/<thread>.pdf` alongside the markdown (the version dir is self-contained for archival). The markdown remains the source-of-truth.

## Output format

Follows `report`'s **markdown source-of-truth + optional PDF** precedent exactly. `<thread>.md` is the primary artifact (diffable, web-publishable — matching the "Mechanics of MobileCoin" web-HTML precedent); `primer-figures` produces an optional `<thread>.pdf` via the same **pandoc-first / LaTeX-opt-in** path `report` uses, reusing `anvil/lib/render.py` and `anvil/lib/render_gate.py` rather than writing new render plumbing. No third rendering path is invented. Teaching diagrams (message flows, lifecycle diagrams, the end-to-end walkthrough) are produced via the documented `mmdc → PNG` path (`report`/`pub` figure primitives) and land under `<thread>.{N}/exhibits/`.

## Command dispatch

| Command | Role | Reads | Writes |
|---|---|---|---|
| `primer` | portfolio/status orchestrator (read-only) | all `<thread>.*` dirs under cwd | (none; reports state per thread + recommends next command) |
| `primer-draft <thread>` | drafter | project `BRIEF.md` (+ optional `spec_ref`), `<thread>/refs/`, shared `research/`; for revisions also prior version + critic siblings | `<thread>.{N}/<thread>.md` + `_progress.json` |
| `primer-review <thread>` | reviewer (pedagogy/prose critic) | latest `<thread>.{N}/`, resolved `spec_ref`, `rubric.md` | `<thread>.{N}.review/` |
| `primer-audit <thread>` | auditor (factual + spec-consistency) | latest `<thread>.{N}/`, resolved `spec_ref`, `rubric.md` | `<thread>.{N}.audit/` |
| `primer-revise <thread>` | reviser | latest `<thread>.{N}/` + BOTH critic siblings | `<thread>.{N+1}/` with `changelog.md`, or reports `AUDITED` |
| `primer-figures <thread>` | figurer | latest AUDITED `<thread>.{N}/` | `<thread>.{N}/exhibits/` + optional `<thread>.pdf` |

## Rubric

See `rubric.md` for the 9-dimension **/44** schema (`anvil-primer-v1`), the **≥35** general advance threshold, the **pedagogy-dominant weighting** (dim 1 *Pedagogical scaffolding / learnability* at weight 7 — the class lives or dies on it), and the critical flags: the two spec-consistency flags (review-side **"Duplicates formal spec section"**, audit-side **"Contradicts cited spec"** — both inactive when `spec_ref` is undeclared), and the technical-accuracy flag (audit-side **"Subtly-wrong intuition"** — a simplification that became *false*, not merely lossy-but-true).

Every critic-writing pass stamps `_meta.json` with `scorecard_kind: "human-verdict"`, `rubric_id: "anvil-primer-v1"`, `rubric_total: 44`, `advance_threshold: 35` (per-review version stamping, issue #346) and writes its sidecar atomically via `anvil/lib/sidecar.py::staged_sidecar` + the per-critic `cleanup_one_staging` sweep (issues #350/#376).

## Project BRIEF artifact type

`primer` is registered as a **skill-identity** `artifact_type` value in the shared project-BRIEF registry (`anvil/lib/project_brief.py::REGISTERED_ARTIFACT_TYPES` / `SKILL_IDENTITY_ARTIFACT_TYPES`; following the #386/#408/#432/#440/#460 pattern). In a shared project BRIEF, a `documents:` entry with `artifact_type: primer` declares that this skill owns the thread. It is NOT a memo subtype: it selects no memo rubric overlay, and memo commands fail loudly when pointed at a `primer`-declared thread.

## Deferred (tracked follow-ups; deliberately NOT in v1)

Per the `anvil:essay` (#460) precedent — ship the artifact class in one right-sized PR, defer the worked example and deeper wiring to follow-ups:

- **The Botho "Botho from the Basics" worked example** under `examples/` — dogfood once the shape lands (Botho #881, the motivating downstream consumer). No vendored example ships in v1.
- **Voice grounding** — a primer's audience is "technically-curious non-specialists" in the abstract, not a named recipient in the author's voice, and dim 6 (*Audience calibration*) already covers "pitched at the stated reader; jargon introduced, not assumed." The `voice:` (#461) contract is NOT consumed in v1. A future consumer wanting persona-consistent primers (a house style across many primers) is a follow-up analogous to `report`'s dim-8 voice suffix — it does not block v1.
- **Consumer-pluggable figure-adapter registry** — `report`'s block-figure adapter (`report.figure_adapters`) is overkill for v1. `primer-figures` ships the `mmdc → PNG` + pandoc path only.
- **LaTeX / TikZ figure path** — v1 ships the `mmdc` + pandoc render path only, matching `report`'s "primary path: pandoc, secondary: opt-in LaTeX" precedent. TikZ-authored teaching diagrams are a follow-up.

## Defaults and overrides

Consumers extend via `.anvil/skills/primer/` in their own repo:

- `rubric.overrides.md` (optional) — additive critical-flag examples; cannot reduce the base rubric.
- `templates/primer.template.md` (optional) — replace the default primer skeleton.

Resolution rule: consumer overrides win when present, else fall back to skill defaults.

## Git sync hook (opt-in, off by default)

Consumers running anvil under an external orchestrator can opt in to a per-phase git commit hook so every lifecycle phase leaves the working tree clean: a repo-level `.anvil/config.json` with `git.commit_per_phase: true` (and optionally `git.push: true`) has each write-bearing primer command end its phase by staging only the dirs it wrote and committing as `anvil(primer/<phase>): <thread>.{N} [<state>]`. The full contract lives in `anvil/lib/snippets/git_sync.md`. All write-bearing primer commands adopt it; the read-only `primer` portfolio orchestrator is exempt by definition. When `.anvil/config.json` is absent or the knob is false, behavior is byte-identical — the hook is **default off**.
