# Audit summary — botho-from-the-basics.1

```json
{
  "critic": "audit",
  "rubric_id": "anvil-primer-v1",
  "audit_clean": true,
  "factual_flags": 0,
  "spec_contradiction_flags": 0
}
```

```json
{
  "spec_ref": {
    "ran": true,
    "declared": "../sections/*.tex",
    "resolved": [
      "whitepaper/sections/01-introduction.tex",
      "whitepaper/sections/02-related-work.tex",
      "whitepaper/sections/03-preliminaries.tex",
      "whitepaper/sections/04-cryptography.tex",
      "whitepaper/sections/05-transactions.tex",
      "whitepaper/sections/06-consensus.tex",
      "whitepaper/sections/07-monetary.tex",
      "whitepaper/sections/08-network.tex",
      "whitepaper/sections/09-security.tex",
      "whitepaper/sections/10-economics.tex",
      "whitepaper/sections/11-implementation.tex",
      "whitepaper/sections/12-governance.tex",
      "whitepaper/sections/13-conclusion.tex",
      "whitepaper/sections/appendix-audit.tex",
      "whitepaper/sections/appendix-formal.tex",
      "whitepaper/sections/appendix-notation.tex",
      "whitepaper/sections/appendix-parameters.tex",
      "whitepaper/sections/appendix-regulatory.tex"
    ],
    "missing": false,
    "contradiction_flags": 0
  }
}
```

```json
{
  "findings": {
    "claims_examined": 82,
    "verified": 72,
    "verified_with_simplification": 10,
    "contradicts": 0,
    "false": 0,
    "major": 1,
    "minor": 5,
    "observations": 3,
    "major_ids": ["F2"],
    "notes": "F2 is a whitepaper-vs-code divergence (ML-DSA-65 role: spec says minting; live code MintingTx has no ML-DSA signature and transaction_pq.rs signs inputs). Operator-facing; the primer correctly follows the declared spec oracle."
  }
}
```
