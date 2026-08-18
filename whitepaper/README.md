# Botho Whitepaper

LaTeX sources for the Botho whitepaper and its two-page executive summary, plus
the Anvil-authored bridge spec that section 11 was integrated from.

## Building

Requires a TeX distribution with `pdflatex` and `bibtex`.

```bash
cd whitepaper
make            # botho-whitepaper.pdf (pdflatex -> bibtex -> pdflatex x2)
make quick      # single pdflatex pass, skips the bibliography
make summary    # botho-executive-summary.pdf
make clean      # remove *.aux/*.log/*.bbl/... build artifacts
make view       # open the built PDF (macOS; `view-linux` for xdg-open)
make watch      # continuous rebuild via latexmk
make wordcount  # approximate word count via texcount
make check      # grep the sources for TODO / FIXME / empty \cite{}
```

Both PDFs are committed alongside their sources. **`botho-whitepaper.pdf` is
also served from the landing page** as
[`web/packages/web-wallet/public/botho-whitepaper.pdf`](../web/packages/web-wallet/public/botho-whitepaper.pdf);
that copy is manual, so
[`.github/workflows/whitepaper.yml`](../.github/workflows/whitepaper.yml)
fails CI when the two files differ. If you rebuild the PDF, copy it across in
the same commit:

```bash
cp whitepaper/botho-whitepaper.pdf web/packages/web-wallet/public/botho-whitepaper.pdf
```

## Layout

| Path | Contents |
|------|----------|
| [`botho-whitepaper.tex`](botho-whitepaper.tex) | Main document — front matter plus one `\input` per section |
| [`botho-executive-summary.tex`](botho-executive-summary.tex) | Standalone two-page overview (self-contained, does not use `preamble.tex`) |
| [`preamble.tex`](preamble.tex) | Shared packages, macros (`\Botho`, `\BTH`, …) and styling |
| [`refs.bib`](refs.bib) | BibTeX bibliography |
| [`sections/`](sections/) | 14 numbered chapters + 5 `appendix-*.tex`, one file per chapter |
| [`figures/`](figures/) | 16 TikZ diagrams (`.tex`, `\input` from the sections) + 3 rendered bridge-flow exhibits (`.png`, `\includegraphics`) |
| [`bridge-spec/`](bridge-spec/) | The Anvil `spec` thread behind section 11 (see below) |
| [`TODO-WHITEPAPER.md`](TODO-WHITEPAPER.md) | Prioritized improvement plan (P0–P3) for the paper |
| `Makefile` | Build targets listed above |

`sections/` maps 1:1 to the chapters of the built PDF, in the order they are
`\input` by `botho-whitepaper.tex`:

`01-introduction` · `02-related-work` · `03-preliminaries` ·
`04-cryptography` · `05-transactions` · `06-consensus` · `07-monetary` ·
`08-network` · `09-security` · `10-economics` · `11-bridge` ·
`12-implementation` · `13-governance` · `14-conclusion`, followed by
`appendix-notation`, `appendix-parameters`, `appendix-regulatory`,
`appendix-formal` and `appendix-audit`.

To add a chapter, drop a `.tex` file in `sections/` and add an `\input` line to
`botho-whitepaper.tex` — the `Makefile` picks up `sections/*.tex` as
dependencies automatically.

## `bridge-spec/` — not part of the whitepaper build

[`bridge-spec/`](bridge-spec/) is a separate
[Anvil](https://github.com/rjwalters/anvil) `spec` artifact, kept here because
its output became whitepaper section 11. It is **not** compiled by the
`Makefile` and does not need to be.

- [`BRIEF.md`](bridge-spec/BRIEF.md) — the Anvil brief: audience, iteration
  budget, and the `code_ref` (`bridge/**/*.rs`) that the consistency audit
  checks every normative claim against.
- `botho-bridge-spec/botho-bridge-spec.{1,2}/` — the numbered drafts
  (`botho-bridge-spec.tex`, plus a `changelog.md` and rendered `exhibits/`
  in draft 2).
- `botho-bridge-spec/botho-bridge-spec.{1,2}.{review,audit}/` — the review and
  code-consistency audit passes for each draft (`verdict.md`, `findings.md`,
  `scoring.md`).
- `botho-bridge-spec/refs/` — the source-of-truth inputs: copies of bridge ADRs
  [0002](../docs/decisions/0002-bridge-custody-scp-validator-federation.md),
  [0003](../docs/decisions/0003-wbth-peg-factor-1-wrapping-and-demurrage-settlement.md),
  [0004](../docs/decisions/0004-bridge-privacy-semantics.md),
  [0005](../docs/decisions/0005-bridge-v1-chain-scope-ethereum-and-solana.md)
  and [0007](../docs/decisions/0007-bridge-import-cluster-tagging.md), plus the
  Mermaid (`.mmd`) sources for the three bridge figures.

The audited body of draft 2 is what lives in
[`sections/11-bridge.tex`](sections/11-bridge.tex); edits to the shipped
section belong there, not in the draft.
