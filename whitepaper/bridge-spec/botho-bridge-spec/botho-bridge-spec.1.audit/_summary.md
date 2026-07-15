# Machine-readable audit summary — botho-bridge-spec.1

```json
{
  "critic": "audit",
  "role": "auditor",
  "rubric_id": "anvil-spec-v1",
  "audit_clean": true,
  "factual_findings": 8,
  "major_findings": 1,
  "code_ref": {
    "declared": "/Users/rwalters/GitHub/botho/bridge/**/*.rs",
    "resolved": true,
    "resolved_file_count": 35,
    "scalar": true,
    "manually_consulted_beyond_glob": [
      "contracts/ethereum/contracts/WrappedBTH.sol",
      "contracts/solana/programs/wbth/src/lib.rs",
      "cluster-tax/src/simulation/bridge_import_sweep.rs",
      "cluster-tax/src/demurrage.rs",
      "transaction/clsag/src/lib.rs",
      "botho/src/ledger/store.rs"
    ]
  },
  "spec_consistency": {
    "ran": true,
    "resolved": [
      "/Users/rwalters/GitHub/botho/bridge/**/*.rs (35 files)",
      "contracts/ethereum/contracts/WrappedBTH.sol",
      "contracts/solana/programs/wbth/src/lib.rs",
      "cluster-tax/src/simulation/bridge_import_sweep.rs",
      "transaction/clsag/src/lib.rs"
    ],
    "missing": false,
    "claims_checked": 42,
    "contradictions": 0,
    "disposition_counts": {
      "spec_wrong": 0,
      "code_wrong": 0,
      "intentional_gap": 4,
      "unregistered": 0
    }
  }
}
```

## Notes on disposition_counts

- `contradictions` = spec_wrong (0) + code_wrong (0) + unregistered (0) = **0** → `audit_clean: true`.
- `intentional_gap` = 4 counts ALL intentional-gap contradictions, every one register-suppressed (import tagging I1, I2; demurrage-settlement D1; and the two constants C4/C5 fold under the import-tagging IMP row — counted here as the import-tagging + settlement divergence set, all registered). `unregistered = 0` (subset that lacked a register row).
- No `implementation_contradicts_spec` critical flag fired: the sole real code-vs-spec divergence set (bridge-import tagging + demurrage-settlement) is register-suppressed with exact Live/Target/Tracking matches.
- The three drafter-flagged focus areas: import tagging = intentional-gap REGISTERED (suppressed); Solana "stubbed" = not present in section, characterization matches code (match); release/bth.rs #856 = live characterization matches (match).
