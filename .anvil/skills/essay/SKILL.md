---
name: essay
description: Draft, review, and revise short-form voice-grounded essays and blog posts (markdown body, 500–1500 words typical) through the canonical anvil lifecycle. Ends at READY with a documented publish handoff; site deploys stay consumer-native.
domain: essay
type: skill
user-invocable: false
---

# anvil:essay — Short-form voice-grounded essays and blog posts

The `essay` skill produces **short-form personal/professional essays and blog posts** (markdown body, 500–1500 words typical) through the canonical anvil lifecycle: `draft → review → revise`, with `revise` looping to `review` until the rubric threshold is met or the iteration cap is reached. The artifact class is grounded in the rjwalters.info adoption survey (issue #460): its native blog skill is a pre-anvil descendant of `paper` (same versioned-dir grammar, older generation — monolithic review.md, 6-dim /30 rubric, single critic, no stamping). Anvil's loop strictly upgrades the machinery; no prior skill fit the artifact — `anvil:memo` is decision-oriented, `anvil:paper` is venue-pinned long-form.

What makes the class distinct is **voice**: the artifact succeeds or fails on whether it sounds like its author. The skill is therefore the first heavy consumer of the voice/persona grounding-docs contract (issue #461, `anvil/lib/snippets/voice_grounding.md`): where memo attaches voice as a calibration suffix on its dim 8, **essay OWNS voice fidelity as rubric dim 2** — the #461-sanctioned "attach to an owned dimension" shape. The essay rubric consumes that contract; it does not redefine it.

## Artifact contract

An **essay thread** is a single essay or blog post authored across one or more revisions, identified by a slug (e.g., `can-claude-read-the-room`, `the-loop-is-the-unit`). Each thread lives inside a **project root** carrying a project-level `BRIEF.md` (the post-#295/#296 canonical model); the body markdown inside each version directory **echoes the slug** (`<slug>.md` — NOT the surveyed consumer's `post.md`):

```
<project>/                     Project root
  BRIEF.md                     Project-level brief (frontmatter `documents:` list +
                               optional top-level `voice:` block per issue #461)
  STYLE_GUIDE.md, VALUES.md,   Typical voice docs (any paths; declared via the
  VOCABULARY.md, corpus glob   BRIEF's `voice:` block — see §Voice grounding)
  research/                    Optional shared evidence pool
  <thread>/                    Thread directory (named for the slug)
    refs/                      Optional reference material (sources, transcripts)
    <thread>.1/                First drafted version (immutable once written)
      <thread>.md              Essay body (filename echoes the slug per #295)
      _progress.json           Phase state for this version
      changelog.md             (revisions only) Maps prior critic notes to changes
    <thread>.1.review/         Reviewer sibling (read-only once written)
      verdict.md               Advance / block + total /44 + critical flags
      scoring.md               Per-dimension scores against rubric.md
      comments.md              Line-level comments keyed to the body markdown
      _summary.md              Machine-readable summary blocks (voice_grounding, …)
      _gate.json               Deterministic pre-flight gate record (see §Gates)
      _meta.json               human-verdict scorecard kind + #346 rubric stamps
      _progress.json           Phase state for the reviewer
    <thread>.1.numeric/        Numeric-consistency gate sidecar (written by the
      _review.json             reviewer's pre-flight via anvil/lib/numeric_consistency)
    <thread>.1.hyperlinks/     Hyperlink-resolver gate sidecar (written by the
      _review.json             reviewer's pre-flight via anvil/lib/hyperlink_resolver)
    <thread>.2/                Revised version (consumes v1 + ALL critic siblings)
    ...
    <thread>.{N}/              Terminal version, marked READY in its _progress.json
```

Versioned dirs (`<thread>.{N}/`) and critic sibling dirs (`<thread>.{N}.<critic>/`) are **immutable once their `_progress.json` records the phase as `done`**. Revisions are produced as a new version dir, never by editing in place.

**Markdown-only body (v1).** There is **no PDF render path**: the publish target is the consumer's site (TSX), not PDF. `anvil:project-share` treats essay threads as source-only (acceptable; deferred). The body filename is `<slug>.md` per the post-#296 canonical model — migration of the consumer's `post.md` corpus is a tracked follow-up via `anvil:project-migrate` + `anvil:rubric-rebackport` once this skill ships.

## Voice grounding (dim 2 — owned)

The skill consumes the issue #461 contract (`anvil/lib/snippets/voice_grounding.md`) directly:

- The project BRIEF's optional top-level `voice:` block declares up to four voice artifacts (`values` / `style_guide` / `vocabulary` / `corpus` glob), resolved via `anvil/lib/project_brief/voice.py::resolve_voice_docs(project_dir)` — project root first, then consumer root.
- **Active tier**: the drafter loads values → style_guide → vocabulary → corpus exemplars, picks 3–5 voice-matched + topically adjacent exemplars, and records them in `_progress.json.metadata.voice_exemplars`. The reviewer scores **dim 2 (Voice fidelity)** against the resolved docs; **every voice deduction MUST quote a corpus exemplar** showing what the target voice sounds like, and the convergence-with-Claude adversarial check applies (*would I, the AI, also write this sentence?* — if yes, scrutinize harder, never defend). The reviewer's `_summary.md` carries the `voice_grounding` block.
- **No `voice:` block** → dim 2 scores WITHOUT calibration AND essay-review records a **`major` finding recommending the block**. Voice is the point of this artifact class, so an absent contract is a defect to surface — not a crash and not a silent pass (the `customer_context.py` posture). This deliberately differs from memo (where an absent block is byte-identical silence): essay's review is still produced, the thread still converges, but the gap is named every pass until the operator declares the contract.
- **Empty block (`voice: {}`)** → inactive, byte-identical to absent (same `major` finding applies — an empty declaration is no contract).
- **Declared-but-missing file** → the tier ACTIVATES; the breakage surfaces as a `major` finding directing the operator to create or fix the file (`resolve_voice_docs` carries `missing: true` entries; never raises).
- **Self-published exclusion (issue #890)**: `essay-review` calls `resolve_voice_docs(project_dir, exclude_self_slug=<thread>)`, which drops `<thread>`'s own published form from the resolved `corpus` before dim 2 is scored against it — the fix for the circular-calibration defect where reviewing a *revision of an already-published* note otherwise calibrates voice fidelity against the artifact under review. Two unioned sources: automatic slug-based inference, and an optional declared `voice_corpus_exclude` on the thread's `documents:` entry for publish-path shapes the automatic rule cannot infer. The exclusion is recorded in `_summary.md.voice_grounding.corpus_excluded` (path + reason) so the calibration base stays auditable, and a `corpus_thin: true` note fires when fewer than 2 exemplars remain after exclusion. See `anvil/lib/snippets/voice_grounding.md` §"Self-published exclusion" and `essay-review.md` step 4.
- The reviser **preserves voice signatures the reviewer flagged as working** — voice-grounded revision must not sand off the persona while chasing rubric points.

The `voice.rhetoric_rules` sub-key (consumer-tunable lint rules, #463's documented integration point) **is wired** (issue #479, porting the memo-render step 4g contract from #468): essay-review step 3c resolves it via `anvil/lib/project_brief/voice.py::resolve_rhetoric_rules(project_dir)` and forwards the path to `lint_rhetoric(extra_rules_path=...)` — the direct-call kwarg, not the gate's `rhetoric_rules_path=`. A declared-but-missing file is still forwarded so the loader's graceful-degrade surfaces the broken declaration as a warning finding; the advisory severity ceiling is untouched.

## State machine

Per-thread state, derived from on-disk evidence (not flags):

```
EMPTY → DRAFTED → REVIEWED → REVISED → … → READY
```

| State | Evidence |
|---|---|
| `EMPTY` | No `<thread>.{N}/` directories exist |
| `DRAFTED` | Latest `<thread>.{N}/` exists with `<thread>.md` (slug-echo per #295) and `_progress.json.draft == done`; no sibling review at the same `N` |
| `REVIEWED` | `<thread>.{N}.review/verdict.md` exists for the latest `N` |
| `REVISED` | A `<thread>.{N+1}/` exists after a prior `<thread>.{N}.review/` |
| `READY` | Latest `<thread>.{N}.review/verdict.md` records `advance: true` AND no unresolved critical flag |

Thresholds: **≥35/44 advances** (general tier — personal/professional voice writing, not the customer-facing ≥39 band; the surveyed consumer's 24/30 gate = 80%, and 35/44 ≈ 80% — same bar). Any critical flag short-circuits regardless of total — block until addressed. Iteration cap: default `max_iterations: 4`; consumer overrides via the project-BRIEF paired override (`max_iterations` + `iteration_cap_rationale`, the #349 memo contract). Exceeding the cap marks the thread `BLOCKED` (human review).

**There is no `AUDITED` state and no figures phase in v1.** The artifact is 500–1500-word markdown prose; the surveyed 8-thread corpus contains no figures. The command set is deliberately `draft / review / revise / status` only.

## Publish handoff contract

**The skill ends at `READY`.** Publishing — the consumer's TSX conversion, post-registry updates, Cloudflare deploy — stays **consumer-native**, exactly as `anvil:report`'s CUSTOMER-READY precedent keeps customer delivery outside the framework. What the handoff guarantees to the consumer's publish tooling:

1. **A `.latest`-resolvable body**: `<thread>/<thread>.{N}/<thread>.md` for the highest `N` (or via a consumer-maintained `<thread>.latest` symlink per `anvil/lib/snippets/version_layout.md`); resolution semantics per `anvil/lib/latest_resolution.py::resolve_latest`.
2. **A READY verdict on that version**: `<thread>.{N}.review/verdict.md` records `advance: true`, total ≥35/44, zero unresolved critical flags — including the convergence-blocking gates below (no unresolved numeric-consistency failure, no broken internal/cross-thread link, no example-coherence flag).
3. **Stamped review metadata**: `<thread>.{N}.review/_meta.json` carries `scorecard_kind: "human-verdict"` plus the #346 stamps (`rubric_id: "anvil-essay-v1"`, `rubric_total: 44`, `advance_threshold: 35`), so downstream tooling can verify WHICH rubric blessed the version without re-reading this skill.

Anything past that boundary (front-matter injection for the site generator, image asset pipelines, deploy) is out of scope by design. A consumer wanting a publish command writes it natively against this contract.

## AI-authorship byline (opt-in, off by default — issue #941)

A project MAY declare a top-level `ai_byline:` block on its `BRIEF.md` (`anvil/lib/project_brief/models.py::AiByline`, resolved by `resolve_ai_byline(project_dir)`) to have `essay-draft` / `essay-revise` append a short, configurable AI-authorship disclosure line to the rendered body. **Strictly opt-in** (`enabled: true` required; absent block or `enabled: false` — the default — is byte-identical to a pre-#941 install, no line is ever rendered). This is an editorial provenance choice made entirely by the consumer, **not** a substitute for, or a claim about, the intrinsic token-level model-output watermark, and it is distinct from the `corpus:` claim-provenance tier (#597) — see `anvil/lib/snippets/provenance.md` for the boundary discussion.

When active, the resolved line (custom `text:` template with `{model}`/`{date}` interpolation, or a generic default) is inserted at the declared `placement:` — `byline` (default; an italicized line directly under the title), `footer` (an italicized closing paragraph), or `frontmatter-only` (recorded in `_progress.json.metadata.ai_byline_text` only, no visible body line — for a consumer whose site template renders its own byline from front matter). See `essay-draft.md` step 3d / step 5 and `essay-revise.md` step 5c for the exact insertion mechanics. The line survives into `SHARE/` and compiled-book output for free, since it is baked into the source `<thread>.md` at draft/revise time, the same way the voice/corpus tiers are.

## Gates — blocking vs advisory

Deterministic pre-flight fires **before** the expensive content review (the framework-wide "deterministic pre-flight before judgment" pattern). The reviewer records every gate outcome in `<thread>.{N}.review/_gate.json` and the blocking outcomes feed the verdict:

| Gate | Mode | Mechanism |
|---|---|---|
| **Numeric consistency** | **BLOCKING** | `uv run --project .anvil python -m anvil.lib.numeric_consistency <version_dir> --write-review --blocking` (issue #462's hook, built for this skill). One `CriticalFlag` per finding-code cluster forces `Verdict.BLOCK` through `critics.compute_verdict`. The detector is the deterministic extraction assist UNDER the full claim-vs-claim semantic LLM pass in essay-review (the blog-review step-2.6 port). |
| **Link audit (deterministic half)** | **BLOCKING** for broken/unresolvable links | the promoted `anvil/lib/hyperlink_resolver.py` (`--write-review`; this skill is the second consumer that triggered the #335 module's promotion from the memo skill-local lib). Unresolved cross-thread refs raise the `critical_broken_cross_thread_anchor` flag; broken internal links are `major` findings essay-review escalates to blocking when unresolved at verdict time. The judgment half ("a named entity needs a link", step-2.7) is essay-review prose → `major`, never critical. |
| **Rhetoric lint** | **Advisory** | `anvil/lib/rhetoric_lint.py::lint_rhetoric` per its fixed #463 warning-ceiling contract. Findings feed dim 9 (*Rhetorical economy*) scoring as deterministic evidence and assist the generic-AI-cadence critical flag, which remains LLM judgment. **Severities are never escalated** — irreducible false positives (quoted material, deliberate style) make the judgment call the reviewer's. |
| **Example coherence** | **BLOCKING via LLM critical flag** | the blog-review step-2.5 prose check carried directly in essay-review (quote the framing sentence + the worked example; state what the example actually needs). No detector — deferred per the #462 gate-1 record until a second observed production failure. |

## Failure-mode catalog (why the gates exist)

- **The toaster failure (example coherence).** Consumer post `2026-05-26-toaster-wants-to-be-good` v1–v3 framed a wall-powered, 800-watt-heating-element toaster as gated by per-token inference *energy*, when its actual gate is cost-of-silicon. The error survived three review passes because the rubric scored craft and no pass asked whether the central example physically needs what the framing claims. Essay-review's example-coherence pass exists because of this failure; it blocks convergence regardless of rubric score.
- **The spread failure (numeric consistency).** Consumer post `2026-05-27-the-loop-is-the-unit` v1 named three finishers at 70 / 56 / 54, then claimed a "70-point spread" (the raw top score leaked into the spread slot) and "sixteen points ahead" (the named gap is 14). The spread error survived to production. The deterministic gate catches the arithmetic shapes; the LLM pass catches the semantic remainder (same quantity stated twice disagreeing, etc.).
- **The well-crafted tangent (thesis unity, issue #1148).** A `##`-level section can score well on every existing dimension — clean transitions (dim 8), the right register (dim 6), a factually verified claim (dim 3), even a protected "What's working" voice signature — while still not belonging in the essay, because none of those dimensions asks whether the section serves the essay's own stated argument. The better the section is written, the more reliably it survives a rubric that scores parts on quality and never tests a part against the whole: a well-transitioned tangent scores *well* on structure, not poorly. Essay-review's 5e thesis-unity pass exists because of this shape — restate the argument in one sentence, test each section against it, and flag (`major`, never critical) any section whose removal would not weaken the argument, even one already protected for its craft.
- **The migrated-corpus first-review failure (issue #881).** `anvil:project-migrate --apply` can leave a legacy single-file `review.md` sitting at the exact canonical `<thread>.{N}.review/` path a real `essay-review` would write to. A bare directory-existence idempotency check cannot tell that occupied-by-foreign-content dir apart from a real prior review, and the sidecar's `FileExistsError` refuse-to-overwrite guard (correctly, in isolation) blocks a naive write into it either way — so the first `essay-review` after a migration had no documented path forward. `essay-review.md` step 1 now checks recognizability (`anvil/lib/critics.py::_has_recognizable_review()`) rather than bare existence, and — when the dir is occupied-but-unrecognized — uses `anvil/lib/sidecar.py::stage_replace` / `commit_replace` / `abort_replace` (a generalization of `project-migrate --adopt-review`'s move-aside/stage/swap recipe) to land a genuine new review while preserving the legacy `review.md` byte-identical alongside it, still discoverable by `anvil:rubric-rebackport` and `project-migrate --adopt-review`. Because `essay-review` is markdown-driven — its `stage_replace` → `commit_replace` window spans many discrete tool calls, so a dead session leaves no handler to call `abort_replace` — step 1 also runs `recover_interrupted_replace()` as a mandatory entry sweep (the `.bak` analog of `cleanup_one_staging`'s `.tmp` sweep), and the fresh-staging primitives refuse outright while an orphaned backup is on disk. Without both, the next run would land a fresh review over the gap and silently strand the legacy file in a hidden `.bak` sibling that nothing sweeps.

## Claim ledger — cross-version verification tracking (issue #1198)

**Failure 3 (#888, "trickling findings"):** a review pass had no record of what had already been checked, so it either re-derived every claim from scratch each round (wasted effort re-verifying unchanged figures) or silently skipped inherited claims it had no way to distinguish from freshly-checked ones. The two critical flags in #888's original report both landed on clauses "carried untouched since `.1` that no pass had ever verified," surfaced only at iterations 4 and 5 — findings that should have consolidated into one revision instead trickled across many small ones.

The fix is a persistent, per-thread `<thread>.{N}/provenance.md` **claim ledger**, extending the existing local-corpus claim-provenance contract (`anvil/lib/snippets/provenance.md`, issue #597) rather than adding a second, parallel file. Full contract, row shape, and the design rationale (including what the killed `_convictions.md` primitive — issues #142/#225/#226/#227/#228 — got wrong and how this avoids it) live in `anvil/skills/essay/lib/claim_ledger.py`'s module docstring.

- **Row shape**: `| Claim | Kind | Source file | Line range | Anchor | Verified against | Verified at | Notes |` — the #597/#868 columns (`Claim`/`Source file`/`Line range`/`Anchor`/`Notes`) unchanged, plus three new ones: **`Kind`** (`corpus` — a #597 corpus-attributed row, unchanged — or `derived` — a claim grounded in `<thread>/refs/`, shared `research/`, a linked repo, or a prior review's prose-logic judgment), **`Verified against`** (what check last validated the row), **`Verified at`** (the version number `N` that check ran at).
- **Activation**: this is a SUPERSET of the #597 corpus tier's activation rule, not a replacement for it. The ledger is written whenever EITHER the corpus tier is active (`corpus:` BRIEF key — existing #597 rows, unchanged) OR the drafter has identified ≥1 load-bearing `derived` claim worth tracking. An essay with zero checkable claims (pure narrative/opinion, nothing corpus-attributed or fact-derived) writes no ledger at all — byte-identical to having nothing to track, the same "nothing declared, nothing written" posture the corpus tier itself already uses.
- **The batching fix**: `essay-review` prioritizes `unverified_rows` (never checked) and `stale_corpus_rows` (a #868 anchor-drift-detected row whose source moved) BEFORE any fresh judgment pass, and folds every priority row into the SAME pass's `comments.md` — not one row per revision cycle. `essay-revise` copies the ledger forward unchanged for every row whose claim text and source are untouched (preserving `Verified against`/`Verified at`), clears `Verified at` on any row whose `Claim` text changed in the revision, and mechanically repoints drifted anchors via `anvil.lib.provenance_anchor` (reused, not reimplemented) — see `essay-review.md` step 4c and `essay-revise.md` step 5b.
- **No verdict-gated branch.** Unlike the killed `_convictions.md` primitive, the ledger's read/write triggers are never conditioned on `verdict.advance` — every reviewed version's claims get checked, whether or not that version ultimately advances. See the module docstring's "What the killed `_convictions.md` primitive got wrong" section for the full comparison.
- **No new critical-flag type, no `convergence.py` change.** An unverified/stale row surfaces as an ordinary `major`/`blocker` finding through the existing `critics.py` aggregation; a fabrication caught via the ledger still raises the existing #597 §Section 6 vocabulary. The ledger changes WHAT a review pass covers each iteration, not WHEN the loop terminates — `convergence.py`'s stable-score resolution order is untouched.

## Command dispatch

| Command | Role | Reads | Writes |
|---|---|---|---|
| `essay` | portfolio/status orchestrator (read-only) | all `<thread>.*` dirs under cwd | (none; reports state per thread + recommends next command) |
| `essay-draft <thread>` | drafter | project `BRIEF.md` (+ `voice:` docs), `<thread>/refs/`, shared `research/`; for revisions also prior version + critic siblings | `<thread>.{N}/<thread>.md` + `_progress.json` |
| `essay-review <thread>` | reviewer (single critic + deterministic gates) | latest `<thread>.{N}/`, voice docs, `rubric.md` | `<thread>.{N}.review/` (+ the `.numeric/` and `.hyperlinks/` gate sidecars via their CLIs) |
| `essay-revise <thread>` | reviser | latest `<thread>.{N}/` + `<thread>.{N}.review/` + gate sidecars + any optional critic siblings | `<thread>.{N+1}/` with `changelog.md`, or reports `READY` |

## Word count (one method — issue #1349)

**Canonical word count (#1349)**: `anvil/lib/word_count.py::essay_word_count` — visible prose only: frontmatter, HTML comments, code fences, images, and link URLs excluded; link text counted; the title (first H1) excluded; other headings counted; tokens are letter/digit runs with internal `'`/`-` (punctuation alone is never a word). Every length envelope (BRIEF `target_length`, the default 500–1500 envelope, dim 9) means this count. `essay-review`, the drafter/reviser self-check, and `rhetoric_lint`'s per-1000-words denominators all use it (`rhetoric_lint` runs the same tokenizer over its scan text, which keeps the title). Compute it with `uv run --project .anvil python -m anvil.lib.word_count <thread>.{N}/<thread>.md`; do not use raw `wc -w`.

## Rubric

See `rubric.md` for the 9-dimension **/44** schema (`anvil-essay-v1`), the **≥35** advance threshold, the voice-dominant weighting (dim 2 *Voice fidelity* at weight 7 — the inverse of memo's substance-dominant tilt), the **load-bearing dim 9** (*Rhetorical economy* absorbs the consumer rubric's length discipline and is fed by the rhetoric lint), and the **seven critical flags** ported from the consumer's blog-review: anti-stance violation, out-of-standing claim, generic AI cadence, factual error, unattributed borrowing, example-coherence failure, numeric-consistency failure.

Every critic-writing pass stamps `_meta.json` with `scorecard_kind: "human-verdict"`, `rubric_id: "anvil-essay-v1"`, `rubric_total: 44`, `advance_threshold: 35` (per-review version stamping, issue #346) and writes its sidecar atomically via `anvil/lib/sidecar.py::staged_sidecar` + the per-critic `cleanup_one_staging` sweep (issues #350/#376).

## Project BRIEF artifact type

`essay` is registered as a **skill-identity** `artifact_type` value in the shared project-BRIEF registry (`anvil/lib/project_brief/types.py::REGISTERED_ARTIFACT_TYPES` / `SKILL_IDENTITY_ARTIFACT_TYPES`; issue #460, following the #386/#408/#432/#440 pattern). In a shared project BRIEF, a `documents:` entry with `artifact_type: essay` declares that this skill owns the thread. It is NOT a memo subtype: it selects no memo rubric overlay, and memo commands fail loudly when pointed at a thread declaring it.

## Deferred (tracked follow-ups; deliberately NOT in v1)

- **rjwalters.info `drafts/` migration** — 8 threads / ~22 version dirs of anvil-adjacent foreign grammar (`post.md` body, monolithic review.md, /30 rubric); needs `anvil:project-migrate` enrollment + `anvil:rubric-rebackport` stamping once this skill exists.
- **PDF render path** — the publish target is TSX, not PDF.
- **Example-coherence detector** — deferred per #462 gate 1 until a second observed failure; the LLM prose check carries it.
- **Audit / figures commands** — no current need for the artifact class; the state machine ends at `READY`.

## Defaults and overrides

Consumers extend via `.anvil/skills/essay/` in their own repo:

- `rubric.overrides.md` (optional) — additive critical-flag examples; cannot reduce the base rubric.
- Voice docs are NOT skill-local config — they are project/persona artifacts declared via the BRIEF's `voice:` block (see §Voice grounding).

## Git sync hook (opt-in, off by default)

Consumers running anvil under an external orchestrator can opt in to a per-phase git commit hook so every lifecycle phase leaves the working tree clean: a repo-level `.anvil/config.json` with `git.commit_per_phase: true` (and optionally `git.push: true`) has each write-bearing essay command end its phase by staging only the dirs it wrote and committing as `anvil(essay/<phase>): <thread>.{N} [<state>]`. The full contract — knob shape, defaults-off rule, commit-message format, staging scope, warn-and-continue failure semantics, and ordering after the `_progress.json` `done` write and the #350 sidecar atomic rename — lives in `anvil/lib/snippets/git_sync.md` (`.anvil/anvil/lib/snippets/git_sync.md` in an installed consumer repo). All 3 write-bearing essay commands adopt it; the read-only `essay` portfolio orchestrator is exempt by definition. When `.anvil/config.json` is absent or the knob is false, behavior is byte-identical — the hook is **default off**.
