# Exact-charge monotonicity proposal for #1962

The compiled source is commit `aa6e11a480af01b4cb1f70511c89da8e48a4cbcd`, tree `d6396756f8cfde3bd0b2887caa058c23fdc90d58`. This later provenance commit only preserves evidence; it is not the source commit compiled by the original run.

[Original Formal (Lean) run 42309103304, attempt 1](https://github.com/botho-project/botho/actions/runs/42309103304) compiled the pure proofs and generated the two records in `generated-new-statements.txt`. Its Sorry, Axiom, proposal-generation and gate-self-test steps passed. Its final Verify self-test failed the real-clean fixture because those two names have no entry in the unchanged record (statement binding rc=1, axiom walk rc=0); the separate verify job was skipped. The overall run failed, and this evidence does not claim green trusted verification.

`provenance.json` pins source hashes, original job/artifact IDs, original ZIP digest, raw output hashes and the two historical compile failures. The original compiled ZIP, source archive, full generated record, raw logs and run-log ZIP are retained outside the issue worktree at the indicated ignored diagnostics directory. This directory's copy of the two generated blocks remains available on the proposal branch after CI artifacts expire.

The records PR appends only these two blocks to `formal/statements.txt`, preserving all existing bytes. No proofs or manifest entries land in that PR; #1963 remains dependent on the records merge. #1711 remains the parent program. An independent reviewer must recompute the blocks through authorized CI against the reviewed original source, rather than copying these printed hashes.
