---
name: ip-search
description: Search a live patent corpus from a thread's inventive-feature inventory and write cited reference summaries into <thread>/prior-art/ in the shape the prior-art positioning critics consume. Degrades gracefully with no API key. A drafting aid, never a clearance search.
---

# `/anvil:ip-search`

Utility skill. Given an ip thread (`anvil:ip-uspto` or
`anvil:ip-uspto-provisional`), derive prior-art queries from its
`BRIEF.md` inventive-feature inventory, query a live patent corpus, and
write one cited reference summary per hit into `<thread>/prior-art/`.

**This is a drafting aid, not a professional or attorney clearance
search.** It is not exhaustive and renders no opinion on patentability,
validity, or freedom to operate — the same posture as the rest of the ip
suite. Say so when you report; every file it writes says so too.

## Usage

```
/anvil:ip-search <thread-dir>
    [--corpus auto|patentsview|uspto]   # default: auto (first key found)
    [--query "<terms>"]                 # bypass BRIEF.md; search these terms
    [--max N]                           # cap references written (default 8)
    [--min-score N]                     # feature-overlap floor (default 1)
    [--dry-run]                         # compute + report; write nothing
    [--force]                           # rewrite references ip-search owns
```

`<thread-dir>` is the **thread root** — the directory holding `BRIEF.md`
and `prior-art/`, e.g. `acme-widget-prov/`. It is **never** a version dir
(`acme-widget-prov.1/`) or a critic sibling
(`acme-widget-prov.1.priorart/`); passing one is refused structurally
before anything is written.

## Preconditions

- An API key in the environment for at least one corpus:
  `PATENTSVIEW_API_KEY` (or `ANVIL_PATENTSVIEW_API_KEY`), or
  `USPTO_API_KEY` (or `ANVIL_USPTO_API_KEY`).
  **A missing key is NOT an error** — see step 3.
- A `BRIEF.md` in the thread root with a `§3 — Inventive features`
  section, *or* an explicit `--query`.

## Procedure

### 1. Run the search

Load the skill lib and call the single entry point. The skill's `lib/`
directory (`anvil/skills/ip-search/lib/` in the anvil source tree,
`.anvil/skills/ip-search/lib/` in a consumer install) is a package whose
`orchestrate` module imports its siblings by relative import, and the
directory name is hyphenated, so it cannot be loaded with a bare
`sys.path.insert` + `import orchestrate`. Use the shared loader
(`anvil.lib.skill_lib_loader`, importable via `uv run --project .anvil`
per the "Running anvil Python from a consumer" pattern):

```python
from pathlib import Path
from anvil.lib.skill_lib_loader import import_skill_lib_module

orchestrate = import_skill_lib_module(
    "ip-search", Path("anvil/skills/ip-search/lib"), "orchestrate"
)
# .anvil/skills/ip-search/lib in a consumer install.

result = orchestrate.run(
    thread_dir,
    corpus=corpus,               # "auto" unless --corpus was given
    query=query,                 # None unless --query was given
    max_references=max_refs,     # 8 unless --max was given
    min_score=min_score,         # 1 unless --min-score was given
    dry_run=dry_run,             # False unless --dry-run
    force=force,                 # False unless --force
)
```

Never pass an API key as an argument — `run()` reads it from the
environment. Do **not** echo a key value into the report, a commit, or a
log line.

### 2. Interpret the status

`result.status` is one of three values, and only one is an error:

| Status | Meaning | Exit |
|---|---|---|
| `ok` | The corpus was queried and the run completed (possibly with zero hits — a legitimate outcome). | 0 |
| `degraded` | No API key, a rejected key, an unreachable endpoint, or an unparseable response. **Nothing was written.** `result.manual_urls` carries the Google Patents fallback. | 0 |
| `error` | An input problem the operator must fix: no `BRIEF.md` and no `--query`, or a thread argument naming an immutable version dir. | nonzero |

`result.success` is True for `ok` and `degraded`. Translate only
`error` into a nonzero exit.

### 3. Report the degraded path honestly

When `status == "degraded"`, do **not** present the run as a failure and do
**not** invent references to fill the gap. Print `result.report`, which
already contains:

- the constructed queries (so the operator can see what was searched for);
- a ready-to-click **Google Patents URL per query**
  (`result.manual_urls`) — the documented manual fallback, since Google
  Patents has no public API;
- the environment variables to set to enable a live corpus.

The thread is left exactly as it was, so `ip-uspto-prior-art` /
`ip-uspto-provisional-prior-art` behave precisely as they do today on a
no-art thread (Dim 5 `null`, "no prior art supplied").

### 4. Report the written references

On `ok`, print `result.report` — it lists the queries issued, a ranked
reference table (slug, number, date, relevance score, title), what was
written, and what was skipped as already collected. Then name the next
command:

```
ip-search: acme-widget-prov/prior-art/ → 2 references written, 1 skipped
next: /anvil:ip-uspto-provisional-prior-art acme-widget-prov
```

Surface `result.warnings` verbatim — they carry the real signal (per-query
corpus failures, zero-result queries, hits dropped below the relevance
floor).

### 5. Do NOT hand-edit the emitted files

The frontmatter shape is the contract the prior-art critics parse. If a
reference proves central, the right enrichment is to paste its **claim
text** under the `## Claim text` section (and add a `claim_text:`
frontmatter key), not to reshape the file. `ip-search` never overwrites a
file it has already written unless `--force` is passed, so annotations are
safe.

## Output shape

One file per reference at `<thread>/prior-art/<slug>.md`, frontmatter
carrying `title` / `inventors` / `publication_date` / `kind` / `summary`
(the fields both prior-art critics document) plus `patent_number` /
`assignee` / `url` / `source` / `retrieved`. See `SKILL.md` for the full
contract, the body sections, and the slug rule.

## Write scope

`<thread>/prior-art/` and nothing else, ever. Version dirs
(`<thread>.{N}/`) and critic siblings (`<thread>.{N}.<tag>/`) are
immutable and are refused before any file is opened. `--dry-run` writes
nothing at all.
