# Elapsed-centroid statement proposal evidence (#2003)

This data branch preserves the genuine proof proposal and generated records. It is not the records PR and must not be merged as the proof implementation.

Compiled source: `c3da7077422cc72bd71273c56f3b98cded305e5d`, tree `fb54ebf01aa180741ffe650360b6c794d2eb044d`, retained unchanged on `feature/issue-2003-proposal`. Base: `8e11b2bba132a8c369c7db97d66ef145a230f7f1`. This evidence commit is later and is not the compiled source identity.

The unchanged normal **Formal (Lean)** workflow, run [42754406833](https://github.com/botho-project/botho/actions/runs/42754406833), built the exact source on pinned Lean 4.35.0. `scripts/check-sorry.sh`, `scripts/check-axioms.sh`, and `scripts/propose-statements.sh` completed successfully. The proposal check first reported the four missing blocks, and its unchanged fallback printed all 249 generated blocks. The original statement-generation step log and extracted outputs are retained here.

The overall run concluded **failure**: the Verify self-test's `real-clean` case rejected exactly the four new names because their blocks are absent from the proposal base record. The independent `verify` job was skipped and `formal-gate` failed. This expected records-first diagnostic is not a successful trusted-verification verdict. No failed source compilation, workflow modification, gate bypass, local Lean/Rust build or placeholder proof occurred.

`source-identity.json` binds the model, manifest, toolchain, gate scripts and source hashes. `run-42754406833/` retains original workflow/artifact ZIPs, raw GitHub run/job/artifact metadata, SHA-256 digests, and archive-member hashes checked against extracted bytes. The artifact archive digest matches GitHub's API digest. All 245 historical compiled blocks match their existing records; the actual records PR preserves every byte of the original file and appends only `generated-new-statements.txt`.

Independent reviewer regeneration is owned by the root reviewer in run 42755505776 on the same immutable source commit. Its actual receipt must be added after completion; this initial data commit does not claim AC3 complete.

Follow-on #2004 must wait for the records-only PR to merge and for these identical four blocks to be present on its base. Then it may use the preserved proposal as the proof source. Pending blocks do not mark the claims proven on main. Part of #1711.
