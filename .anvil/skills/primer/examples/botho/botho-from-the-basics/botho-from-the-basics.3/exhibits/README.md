# Exhibits — mermaid sources only, no rendered PNGs (intentional)

This directory ships only the five mermaid `.mmd` sources for the primer's
teaching diagrams:

```
fig1-stealth-address-flow.mmd
fig2-hybrid-pq-envelope.mmd
fig3-scp-mining-decoupling.mmd
fig4-anti-hoarding-money-flow.mmd
fig5-capstone-payment-timeline.mmd
```

The rendered PNGs (`figN-*.png`) that a real `primer-figures` run produces
here, and that `../botho-from-the-basics.md`'s five inline
`![Figure N — …](exhibits/figN-*.png)` references point at, are **not
vendored**. This is a deliberate trim for the shipped worked example, not a
missed commit:

- The original dogfood run (see `../_progress.json`
  `metadata.figures_carried_forward` / `metadata.figures`) genuinely
  rendered and embedded all five PNGs into a PDF.
- Vendoring that PDF and the ~1.1 MB of full-resolution PNGs would blow the
  size envelope every other vendored anvil example runs in (~64–156 KB); see
  `../../../expected-thread.N/README.md` § "Provenance and what was
  trimmed" for the full rationale, and the pinned regression test
  `anvil/skills/primer/tests/test_primer_example_brief_parses.py::test_no_full_resolution_exhibit_pngs_vendored`.

**If you are auditing this example for broken links** (#1184): the body's
five `exhibits/figN-*.png` references not resolving in this tree is the
expected, documented state — please do not re-file it as a fresh bug.  If
you want to see the rendered diagrams, render the `.mmd` sources locally via
`mmdc` (`anvil/lib/render.py::render_mermaid_to_png` is the canonical
wrapper; pass `anvil/lib/figures/mermaid-theme.json` as the `-c` config to
match the anvil palette).
