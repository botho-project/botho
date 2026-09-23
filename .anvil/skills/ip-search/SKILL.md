---
name: ip-search
description: Prior-art search utility for the ip suite — derives queries from a thread's inventive-feature inventory, queries a live patent corpus (PatentsView / USPTO Open Data Portal, Google Patents as a documented manual fallback), and writes cited reference summaries into <thread>/prior-art/ in the exact shape the prior-art positioning critics already consume. Degrades gracefully with no API key. A drafting aid, never a clearance search.
domain: anvil
type: skill
user-invocable: true
---

# anvil:ip-search — Prior-art search for the ip suite

`ip-search` is a **utility skill** (alongside `anvil:project-scout`,
`anvil:project-photos`, `anvil:project-share`, `anvil:project-book`,
`anvil:help`, `anvil:deslop`, `anvil:diff`). It is the missing input half
of the prior-art workflow the two ip skills already ship the output half
of.

```
/anvil:ip-search <thread-dir>
    [--corpus auto|patentsview|uspto]
    [--query "<terms>"]      # bypass BRIEF.md and search these terms
    [--max N]                # cap reference files written (default 8)
    [--min-score N]          # feature-overlap floor (default 1; 0 keeps all)
    [--dry-run]              # compute + report; write nothing
    [--force]                # rewrite references ip-search already owns
```

## Why it exists

Both prior-art positioning critics — `ip-uspto-prior-art` (dim 5, §102/§103)
and `ip-uspto-provisional-prior-art` (dim 5, positioning) — state their own
non-scope plainly:

> This critic evaluates the application against prior art the **operator has
> supplied** … It does **not** perform its own patent search. … A future
> skill (potentially `anvil:ip-search`) may address it.

The consequence in production (canary, issue #957): prior-art coverage
depended entirely on hand-collected desk research, and the positioning
critic carried a standing advisory that references existed only as BRIEF
characterizations with no per-reference summary. For a legal artifact whose
whole value is priority scope, an actual search capability is the missing
half. This skill is that half — and *only* that half. It finds and
summarizes; the critic still owns every judgment.

## Posture — what this is NOT

**`ip-search` is a drafting aid, not a professional or attorney prior-art
clearance search.** Same posture as the rest of the ip suite. It is not
exhaustive, it queries US corpora only, it does not read claims, it applies
no classification (CPC/IPC) strategy, and it renders no opinion on
patentability, validity, or freedom to operate. Every reference file it
writes carries that disclaimer in its body, and so does every run report.
Have counsel run a real search before relying on the positioning.

## What it does

```
<thread>/BRIEF.md                    ← §3 inventive-feature inventory
        ↓  brief_features
one Feature per inventive feature    ← ranked query vocabulary
        ↓  query
one SearchQuery per feature + union
        ↓  corpus  (PatentsView / USPTO ODP)
deduplicated, feature-scored hits
        ↓  reference
<thread>/prior-art/<slug>.md         ← the critics' documented shape
```

Queries are derived from the **same inventory the `s112` critic scores the
spec against** (`§3 — Inventive features`, the disclosure denominator), so
what the search looks for and what the disclosure must distinguish itself
from are the same list by construction.

## Output contract

One markdown file per reference at `<thread>/prior-art/<slug>.md`. The
frontmatter emits **exactly the field names the prior-art critics document
as the format they parse** — `title`, `inventors`, `publication_date`,
`kind`, `summary` — plus the provenance superset issue #957 requires:

```yaml
---
title: "Split-path excitation network for a piezoresistive bridge sensor"
inventors:
  - "Marion Smith"
  - "Kai Nakamura"
publication_date: "2019-04-16"
kind: "patent"
summary: "A pressure sensor includes a piezoresistive bridge driven by …"
patent_number: "US10261234"
assignee: "Northline Sensors Inc"
url: "https://patents.google.com/patent/US10261234"
source: "anvil:ip-search/patentsview"
retrieved: "2026-01-15"
---
```

A YAML mapping the critics read **by key** is unaffected by keys they do
not look up, so the superset is consumed with no reformatting.
`claim_text` is deliberately **omitted** rather than emitted empty: neither
corpus returns claim text, and an empty `claim_text:` would read as "this
patent has no claims". The body says so explicitly instead.

Body sections, in order:

| Section | Content |
|---|---|
| `# <number> — <title>` | Heading |
| `## Bibliographic data` | number, kind, date, inventors, assignee, **cited URL** |
| `## Summary` | The corpus abstract (or an explicit "no abstract returned" note) |
| `## Relevance to <thread>` | Per-feature term-overlap table, tied to the brief's own `3.1` / `3.2` ids |
| `## Claim text` | Not retrieved — how to add it if the reference proves central |
| `## Provenance` | Corpus, which queries matched, retrieval date |
| Disclaimer | The not-a-clearance-search footer |

The relevance note is **mechanical term overlap**, deliberately: it is
retrieval evidence, not a positioning verdict. Manufacturing an LLM
judgment here would put an unreviewed opinion inside the evidence the
positioning critic is supposed to form its own opinion from.

## Corpora

| Corpus | Endpoint | API key | Notes |
|---|---|---|---|
| `patentsview` (primary) | `https://search.patentsview.org/api/v1/patent/` | `PATENTSVIEW_API_KEY` (or `ANVIL_PATENTSVIEW_API_KEY`) | Granted US patents; title/abstract/date/inventors/assignee in one call. Free key on request. |
| `uspto` (secondary) | `https://api.uspto.gov/api/v1/patent/applications/search` | `USPTO_API_KEY` (or `ANVIL_USPTO_API_KEY`) | USPTO Open Data Portal; covers published applications as well as grants (`kind: publication`). Parsed defensively — the ODP payload shape has moved more than once. |
| Google Patents | — | — | **Manual fallback only.** No public API; scraping is against its terms. `ip-search` emits ready-to-click search URLs instead. |

`--corpus auto` (the default) takes the first corpus with a key configured,
in the order above.

## Graceful degradation with no API key

**A missing key is a supported mode, not an error.** With no key configured
(or a key the corpus rejects, or an unreachable endpoint, or a response the
adapter cannot parse), `ip-search`:

1. writes **nothing** — it will not fabricate reference files from
   unverified data;
2. prints the constructed queries and a ready-to-click **Google Patents URL
   per query**, so the operator can run the same search by hand;
3. names the environment variables to set;
4. exits **0**. The run reports `status: degraded`.

The thread is left exactly as it was, so Dimension 5 behaves precisely as
it does today on a no-art thread (`null`, "no prior art supplied") — no
regression from the pre-`ip-search` path.

## Write scope (a hard contract)

The **only** directory `ip-search` ever writes is `<thread>/prior-art/`.
This is enforced structurally, before any file is opened:

- `prior_art_dir()` **refuses** a thread argument that names an immutable
  version dir (`<thread>.{N}/`) or critic sibling
  (`<thread>.{N}.<tag>/`) — the run returns `status: error` having written
  nothing.
- `assert_write_target()` refuses any path that is not a *direct child* of
  `<thread>/prior-art/` (no `../` escape, no nested subdir).
- A tree-hash test asserts every byte outside `prior-art/` is unchanged by
  a full run.

**Existing reference files are never overwritten** without `--force`.
Operators hand-annotate these files (pasted claim text, positioning notes),
and a re-run must not silently discard that work:

- A patent an existing file already covers — detected by scanning existing
  files for the publication number, so hand-written references count too —
  is **skipped**, not re-collected under a new name.
- A *different* patent whose natural slug is taken gets a numeric suffix
  (`smith-2019-2.md`), so nothing is clobbered either way.
- `--force` rewrites in place the files `ip-search` itself owns.

A re-run with no new art therefore writes nothing at all.

## Slugs

`<first-inventor-surname>-<year>` — matching the `smith-2019` /
`jones-2021` slugs the critics' own positioning-matrix examples use —
falling back to the assignee's first token, then the publication number.
Diacritics fold; collisions take a numeric suffix.

## Relevance floor

By default a hit whose title and abstract share **no** inventive-feature
vocabulary is dropped and named in the report (`--min-score 0` keeps
everything). A corpus indexes full text, so a zero-overlap hit is a weak
signal, and writing it costs the operator a deletion and costs the
positioning critic a junk row in its matrix.

## Determinism

Query construction and ranking are pure functions of the brief: term
ranking is label-order-then-frequency-then-alphabetical (a total order),
and hits sort by `(-score, patent_number)`. Two runs against the same
corpus response therefore rank and slug identically. The only
non-deterministic field in an emitted file is the `retrieved:` date, which
is genuine provenance for a legal artifact — and because a re-run never
rewrites an existing file, it never churns.

## State machine

No versioned artifact, no critic sibling, no rubric. `ip-search` is a
read-mostly utility: the on-disk evidence is the reference files the
operator asked for. It does **not** advance any thread's state, and it does
not run the positioning critic — after a search, the operator runs
`ip-uspto-prior-art` / `ip-uspto-provisional-prior-art` as usual, which now
has art to score against.

## Out of scope (v1)

- **Claim-text retrieval.** Neither corpus returns claims in the search
  response; a per-hit full-document fetch is a follow-up.
- **CPC / IPC classification strategy.** Real search practice starts from
  classification, not keywords. Keyword-from-features is the honest v1.
- **Non-US corpora** (Espacenet / EPO OPS, WIPO PATENTSCOPE).
- **Non-patent literature** (the `kind: publication` / `kind: product`
  arms of the critics' taxonomy are emitted only when a corpus reports a
  published application; NPL search is not attempted).
- **Any automated positioning verdict.** The critics own dim 5, and they
  keep owning it.

## Lib primitives composed

Skill-local `lib/` (per CLAUDE.md's "skill-local first, lib promotion
later"): `brief_features.py` (brief → inventive-feature inventory + ranked
term vocabulary), `query.py` (features → deterministic queries + manual
fallback URLs), `corpus.py` (stdlib `urllib` clients + API-key resolution +
the degradation contract), `reference.py` (frontmatter/body rendering,
slugging, relevance scoring, and the write-scope guard), `orchestrate.py`
(single `run()` entry).

`corpus.py` follows the `anvil/lib/cite.py` precedent for external-API
integration: stdlib only, explicit `User-Agent`, bounded exponential
backoff, and **no live network in tests**.

## Tests

Fixtures are programmatic builders in `tests/_ip_search_fixtures.py`;
recorded corpus responses live in `tests/cassettes/`; the lib loads under
the unique package name `ip_search_lib` via
`tests/_ip_search_skill_lib.py` (the #362/#367 cross-skill collision
pattern). Files (per the #58 distinct-filename convention):

- `test_ip_search_features.py` — brief parsing (all three fallback tiers),
  deterministic term ranking, query construction, the `--query` bypass.
- `test_ip_search_corpus.py` — cassette-driven client tests for both
  corpora (request shape, key header, field mapping) and every degradation
  path (no key, rejected key, unreachable, malformed body, zero hits,
  partial multi-query failure). One opt-in live smoke test, gated on
  `ANVIL_IP_SEARCH_LIVE=1` **and** a real key, so the default run — from
  the repo root or standalone — never touches the network.
- `test_ip_search_reference.py` — the frontmatter contract parsed back with
  `yaml.safe_load`, the provenance superset, escaping, slugging, relevance
  scoring, disclaimer presence, byte-stable rendering.
- `test_ip_search_writescope.py` — the structural guard against version
  dirs / critic siblings / path escapes, plus a SHA-256 tree hash proving
  zero mutation outside `prior-art/` and that `--dry-run` writes nothing.
- `test_ip_search_orchestrate.py` — end-to-end `ok` / `degraded` / `error`,
  no-overwrite discipline, re-run idempotence, corpus selection.
