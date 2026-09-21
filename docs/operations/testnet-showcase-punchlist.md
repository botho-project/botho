# Full testnet showcase: review and punch list

**Review date:** 2026-09-21. **Baseline:** [`6dbcb92463214694f3122a05c9c48986d4f4f1b7`](https://github.com/botho-project/botho/tree/6dbcb92463214694f3122a05c9c48986d4f4f1b7).
**Tracker:** [#1373](https://github.com/botho-project/botho/issues/1373).
**Scope selected by the owner:** full showcase, including confidential amounts and advanced privacy; web wallet and MetaMask are primary demonstration clients.

**Assessment: not ready for that showcase.** The main obstacles are an unhealthy deployed network, unfinished confidential-transaction integration, and gaps between the shared browser signer and the native wallet. UI polish alone will not satisfy this scope. Several substantial components already work; their local and CI evidence must be carried into an integrated, versioned release candidate.

This is an engineering review and proposed acceptance plan, not a complete cryptographic audit. It covers merged source, public read-only observations and selected local checks. Open worktrees are not treated as delivered. Prior work's measurements are attributed to their recorded experiments. No live send, faucet grant, deployment, authority change or reset was performed.

## 1. What a successful demonstration must show

Use one identified testnet genesis and release candidate, two independently controlled wallets, and a third native wallet for interoperability. Retain transaction IDs, block hashes, software versions and sanitized logs for the run.

1. A fresh browser loads the deployed app; the fleet agrees on the canonical tip and actually confirms transactions. A working faucet funds a new wallet.
2. The user installs/connects the specified MetaMask Snap through a visible product flow. Receive addresses, network identity, fees and approval dialogs are understandable without developer-console commands.
3. Web sends a confidential payment to Snap. Snap discovers it, reports the correct balance, and spends it back. A native wallet participates in a second round trip. Verify recipient value, change, fees and final balances against accepted ledger transactions.
4. Inspect the serialized transaction, RPC response and explorer together: ordinary input/output values and recipient openings are not public; authorized recipients recover them. Public fees, provenance metadata, lottery relationships and node-trust assumptions are explained accurately.
5. Demonstrate an actual fee-funded lottery award, followed by independent accepted spends of its source and repeated/nested awards. Show fees, burn, pool carryover and supply accounting. Do not credit an undiscoverable or unspendable award as spendable balance.
6. Reload/lock/unlock both clients, restore from the documented backup in a fresh profile, and switch ingress. The wallet recovers the same funds without mixing chains, duplicating payments or displaying another wallet's balance.
7. Exercise rejection, connection loss, stale fee/proof context and an interrupted submit. The UI distinguishes rejected, pending and confirmed states; retry does not create a second payment.
8. Demonstrate the chosen network-privacy path and document what the ingress can observe. A separate controlled load run establishes its latency and memory limits; a successful single payment is not a capacity result.

Ethereum/Solana bridge round trips are a separate lane below. If included in the presentation, they must also pass; unfinished bridge/hosting/DeFi paths should be explicitly marked unavailable in the demo navigation.

## 2. Current deployment: fresh observations

Public RPC was queried with normal TLS validation on **2026-09-21 22:22:48 UTC**, then seed/faucet status was sampled again at **22:24:48 UTC**. The [retained evidence](testnet-showcase-evidence/2026-09-21.json) includes responses and observation limits.

| Endpoint | Observed result | Demo implication |
|---|---|---|
| `seed.botho.io` | Height 2634; unchanged tip on repeat; minting false; reports synced | Reachability/synced alone is insufficient evidence of liveness |
| `seed2.botho.io` | TLS verification fails: expired certificate | A built-in ingress cannot be used normally |
| `faucet.botho.io` | Height 2634; minting true; `slotStalled:true`; `synced:false`; reported stall exceeds 921,000 seconds | Diagnose consensus/production before relying on faucet confirmation |
| `eu.seed.botho.io`, `ap.seed.botho.io` | Height 2634, same tip as seed/faucet, minting false | Regional endpoints answer; this does not establish advancing consensus |
| Reachable fleet builds | `8f498a8-dirty`, build time 2026-07-16, version 0.6.0 | These are not the reviewed September source/artifacts |
| Faucet status | Enabled; 1 BTH/request; lifetime dispensed reported as zero | Status is not evidence of a completed grant |

A fresh Chrome context loaded `/`, `/wallet` and `/network` with HTTP 200 and no captured page exceptions. The network page displayed height 2634, 4/5 nodes in sync, one unreachable node and a stalled faucet slot. The landing page advertised a live testnet and `<5s` finality. These observations do not establish that a send works or diagnose the stall's cause. The two-minute unchanged tip plus the node's stall report warrant a liveness investigation; on-demand minting by itself is not a failure.

[#1051](https://github.com/botho-project/botho/issues/1051)'s historical height-202/cap explanation must be refreshed with these observations. Do not assume the current stall has the same cause or repair it by changing monetary parameters without review.

## 3. Prioritized punch list

**P0** blocks the selected full showcase. **P1** is required for a smooth, repeatable public demonstration. Owners below are roles to assign, not claims that someone has accepted the task.

| ID / priority | Work and owner | Acceptance and existing tracking |
|---|---|---|
| D1 / P0 | **Restore a known deployment and liveness.** Operator + consensus engineer | Renew/verify TLS for all five ingresses; diagnose stalled slot/quorum/minting; pin clean build SHA, features, protocol, genesis and configuration. Record accepted transfers across the fleet and an agreed observation window of healthy operation. Update #1051. Activation of new protocol rules follows D4, not a routine binary update. |
| D2 / P0 | **Close CT policy and construction decisions.** Protocol/economics reviewers | Resolve remaining D1 fee buckets, D3 circulation/factor policy and base-fee choice using actual selector/affordability evidence. Preserve ratified EpochOrigin. Publish an independent review of combined charge/range/conservation/CLSAG commitment linkage and its full-domain bounds. #902, #1306, #1307, #1372. |
| D3 / P0 | **Implement the CT output/opening and memo codec across runtimes.** Crypto + wallet engineers | Canonical, versioned hybrid AEAD records; one set of native/WASM/mobile vectors; authenticated network/genesis/index/commitment/context binding; exact binary bridge-memo round trip; malformed or wrong-context records rejected. No plaintext ordinary amounts or unauthenticated classical memo fallback in CT. #1308, #904. |
| D4 / P0 | **Integrate CT and finish LotteryV2 through every consumer.** Node/storage/RPC + client engineers | Actual mempool→producer→validator→ledger execution; authenticated payout contexts and body proofs; snapshots/reopen/recovery; explicit old-client rejection; accepted independent source/repeated/nested payout spends in web and Snap as well as native. Record legacy-claim disposition and a reviewed activation/genesis plan. #1309, #1286; merged #1365 completes only the inactive native slice. |
| D5 / P0 | **Make proof cost fit the chosen confirmation schedule.** Performance + consensus engineers | Full serialized CT generation, propagation and verification on browser/Snap and minimum supported native hardware; 1–16 input/output cases, worst allowed tags/proofs, invalid-proof rejection, byte/work admission bounds, and bounded multi-node load. Resolve exact-height freshness when user approval/proving spans blocks. #1310. |
| D6 / P0 | **Repair browser/Snap decoy, input and fee parity.** Shared signer + wallet engineers | Replace deterministic pool-order decoys with the reviewed selection policy; enforce maturity and real-input exclusions; use authenticated metadata and the selected CT fee statement. Capture successful fresh, old/high-factor, scarce-decoy and multi-input payments from each client. Confirmation shows the fee actually signed, including dust/shape changes. #1306 has measurements, but production client parity still needs an implementation task. |
| D7 / P0 | **Keep the displayed balance tied to the user's wallet.** Web wallet engineer | A failed WASM scan/RPC call displays an explicit unavailable/stale state. Never substitute `wallet_getBalance` from the remote node. Failures must not become an empty history indistinguishable from no activity. Add a regression using deliberately different local-wallet and node-wallet balances. New bounded task. |
| D8 / P0 | **Deliver a usable MetaMask entry point and successful sends.** Web/Snap engineer + release owner | Visible install/connect/receive/send flow; persist an explicitly chosen ingress; pin tested Snap ID/version/artifact and installation mode; run the production Snap's successful funded send in SES against a real local node, then web↔Snap live CT round trips and recovery. #1089; new UI/local-positive-test children are needed. |
| D9 / P0 | **Bind client state to chain identity.** Shared adapters + clients | Reject missing/unrecognized network identity; compare genesis/protocol/capabilities, not only a network name. Version owned-output caches, authenticate checkpoints and rescan safely after an approved chain change. Preserve wallet keys; cover a reset with the same network name, ingress switches and stale asynchronous results. Coordinate #1309 and desktop #1371. |
| D10 / P0 | **Review and correct the cryptographic claims.** Protocol/security + whitepaper reviewers | Address the recipient-unlinkability proof gap, old non-hybrid formulas, public-amount anonymity claims, quantum spend limitations and memo security. Publish a claim→assumption→implementation→evidence matrix; correct both source and shipped PDF. Details in section 5. New review task, linked to #904/#1308. |
| D11 / P0 | **Define and evidence network privacy.** Network + wallet engineers | Trace RPC submit through propagation; document ingress visibility and remove unintended ownership disclosures from fee estimation. Demonstrate the actual configured relay path with bounded observations. If claiming WebRTC/obfuscation, integrate and exercise the transport rather than inferring deployment from module tests. New integration/review task. |
| D12 / P1 | **Share a resumable browser scan pipeline.** Web/shared signer engineer | Reuse authenticated output data for balance/history/send, window and checkpoint scans, expose progress, prevent concurrent whole-chain refreshes, and bound cancellation/timeout behavior. Measure cold/warm loads at a representative height. Snap already has incremental balance/history scanning; preserve that work. |
| D13 / P1 | **Make receiving and transaction history presentable.** Web/Snap UX engineer | Demonstrate a supported copy/share/deep-link path for ~4.4 KB v2 addresses; do not promise a single address QR. Record outgoing receipts with real tx hash/fee/status, distinguish change and incoming payments, and verify fee precision, pending states, errors and recovery. Extend existing address/claim/payment-request coverage. |
| D14 / P1 | **Freeze, rehearse and archive the demo release.** Release + test engineer | One manifest binds node/web/WASM/Snap/whitepaper builds, genesis, supported browsers and policies. CI runs the real local cross-client happy path plus failure cases. A deployed dress rehearsal records each step in section 1 and a recovery run. Refresh stale readiness docs and track remaining findings explicitly. |

### Execution order

Run deployment diagnosis and the bounded wallet fixes while the CT design gates progress. The protocol dependency chain is:

```text
D2 policy + independent construction review
  → D3 codec + D4 consumers/LotteryV2 integration
  → D5 full-cost and multi-node acceptance
  → explicit activation/genesis decision
  → D1 deployment of that candidate
  → D14 full deployed rehearsal

D6–D9 client correctness + D10 claims + D11 network privacy
  → integrated into the same candidate before D14
```

Do not wait for public testnet recovery to add a successful Snap test: a funded real local node can exercise the production Snap interface now. Do not block that test on npm publication either; choose and record the supported test installation mode. Public distribution and production Snap identity remain separate release decisions.

The first small implementation tasks should be D7 (wrong-wallet fallback), D8's successful local Snap send plus connection surface, and D9 (strict identity/cache contract). D6 changes a privacy-sensitive policy shared with native clients and needs review rather than an arbitrary shuffle substitution. Protocol engineering continues under #1306–#1310/#1286.

## 4. Client review: what exists and what is missing

### Web wallet

**Present:** encrypted local wallet/contacts/claim storage, backup/import, network selection, v2 hybrid address derivation and signing, send/receive/payment links, and a real-local-node Playwright send harness. Focused checks during this review passed: web-wallet TypeScript, and **40 tests across wallet context, network configuration and receive modal**. This is not full browser financial acceptance.

**Concrete gaps:**

- [`fetchBalance`](../../web/packages/web-wallet/src/contexts/wallet.tsx) catches a client-side scan failure and calls `adapter.getBalance([address])`. [`RemoteNodeAdapter.getBalance`](../../web/packages/adapters/src/remote.ts) ignores that address and calls the server's `wallet_getBalance` with no wallet argument. This can display the node's wallet balance as the browser user's. `fetchHistory` separately turns scan errors into `[]`. D7 should make these states explicit.
- [`spendableBalance`, `buildOwnedHistory`, `buildSendTransaction`](../../web/packages/wasm-signer/src/send.ts) each request outputs from height zero to tip. Balance/history refresh together on new blocks or polling; send scans again. `getRawOutputs` makes one range call. This creates redundant network traffic and KEM scanning as the chain grows. Measure and replace the repeated work, without changing decoy availability accidentally.
- The shared send builder selects rotating consecutive windows from the RPC-ordered decoy pool. Rust shuffling of ring position does not randomize the chosen set. Native's gamma selector, standalone CLI's age-window selection and browser selection are different; #1306 correctly records that distinction. A ring size of 20 is not evidence of equal anonymity across clients.
- Fee estimation passes the wallet's owned target keys to `cluster_getWealthByTargetKeys`. That reveals an ownership grouping to the selected ingress, even though spend keys never leave the browser. CT fee integration needs a privacy review of the whole RPC transcript, including spent-key-image queries.
- `validateRpcEndpointForNetwork` accepts missing `health.network` because it tests mismatch only when the field is truthy. Addresses are currently intentionally testnet-only. Treat a recognized network, protocol and genesis as an explicit connection contract.
- [`SafeQR`](../../web/packages/web-wallet/src/components/SafeQR.tsx) already avoids oversized-QR crashes and offers copying. The v2 address carrying KEM+DSA keys does not fit a single QR. This is a demo interaction constraint, not a missing crash fix.

### MetaMask Snap

**Present:** SRP-derived keys, WASM under SES, encrypted persisted scan state, incremental balance/history scanning, contacts, claim links, payment-request parsing, localization and the scoped July key-handling review. #1089's latest reconciliation comment correctly records children #1091–#1096 as closed; its older body/Phase-1 wording should not drive a duplicate backlog.

**Concrete gaps:**

- No `wallet_requestSnaps`/`wallet_invokeSnap` integration was found in the reviewed web app. The Snap exposes RPC-driven dialogs but its manifest does not provide a wallet home-page flow. The package is still `private: true`. Build a visible connection path and freeze the ID: changing Snap identity changes entropy-derived wallet identity.
- [`send.snap.ts`](../../web/packages/snap/test/send.snap.ts) checks approval/rejection, empty-chain/no-funds and malformed input. It does **not** establish a successful submitted/mined production-Snap payment. The spike's historical success cannot replace this test.
- `rpcUrl` is still supplied per request. [`assertNetworkAllowed`](../../web/packages/snap/src/node.ts) permits an absent network field. Persisted [`ScanState`](../../web/packages/snap/src/state.ts) is keyed by schema/network name and height, not genesis. Its shallow-range reconciliation is useful but does not identify a new chain reusing `botho-testnet`.
- Send still calls the shared whole-chain builder. Its default fee is `wasm.minFee()` rather than a quote bound to selected inputs and current consensus economics. A green dialog test does not validate fee adequacy or block acceptance.
- History projects owned outputs into received/spent facts; it is not a complete outgoing receipt record. Preserve meaningful distinctions and show confirmation evidence without guessing the spending transaction from a ring.

The [July key-handling report](../../audits/2026-07-20-snap-keyhandling.md) is useful scoped evidence. Its exclusions include Rust/WASM internals and live acceptance; it is not a new audit of the CT stack.

### Native/desktop and other consumers

Merged #1369 repairs standalone wallet feature builds. #1371 is still implementing desktop hybrid keys/network-specific addresses; #1367 and #1327 track runtime smoke/minimum-platform evidence. Treat these as active dependencies if the desktop is shown. Native, standalone wallet, WASM/Snap and mobile must consume the same final CT/lottery records even if only three appear on stage. Bridge watchers, snapshots, explorer and view-only/export APIs are also consumers; a classical view key alone must not be advertised as a complete hybrid-scanning credential without testing its actual capability.

## 5. Whitepaper and cryptographic review

### C1. Recipient unlinkability proof uses the wrong security game — high-priority review finding

In [cryptography §4, “Recipient Unlinkability”](../../whitepaper/sections/04-cryptography.tex), the proof asks an IND-CCA challenger for a challenge over two recipient public keys and then constructs an output using the hidden branch's `K_b` and `B_b`. Ordinary KEM IND-CCA security distinguishes a real shared key from a random key for a challenge encapsulation; it does not supply the two-recipient anonymity oracle assumed here. The simulation as written also assumes access to the hidden recipient branch.

A secret KEM key input can support a pseudorandom stealth scalar argument, but that alone does not prove that the **published KEM ciphertext** hides which public key received it. Recipient/key privacy needs its own game and assumptions, with the exact hybrid construction, public metadata, multi-user queries, corruption scope and quantum oracle treatment included.

This is a confirmed problem in the written argument, **not a demonstrated attack on Botho or ML-KEM**. The repair is a precise, independently reviewed construction argument, or a qualified claim until that argument exists. [NIST FIPS 203](https://csrc.nist.gov/pubs/fips/203/final) specifies ML-KEM key establishment. [Grubbs, Maram and Paterson, “Anonymous, Robust Post-Quantum Public Key Encryption”](https://eprint.iacr.org/2021/708) treats anonymity as a distinct requirement; its historical Kyber analysis is not, by itself, a verdict on final ML-KEM or this protocol.

### C2. Claims need to distinguish primitives, complete transactions and deployment

- [Security §9](../../whitepaper/sections/09-security.tex) still writes the older KEM-only `Hs(K || index)` recipient formula, while §4 and ADR 0008 use the hybrid DH+KEM input. Reconcile equations, definitions, proof statements and test vectors.
- Active [`TxOutput.amount` and `ClsagRingInput.pseudo_output_amount`](../../transaction/clsag/src/lib.rs) are public; verification constructs zero-blinded commitments. This defeats a present-tense claim of hidden transaction amounts and exposes amount-based decoy filtering. The existing CT caveat in §12 does not repair unconditional claims elsewhere.
- Pedersen commitment hiding assumes appropriate random blinding. It does not establish information-theoretic privacy for a complete transaction carrying encrypted openings, fees, range proofs and public provenance. Binding/soundness and classical spend authorization have different assumptions; keep those separate from recipient PQ privacy.
- The `1/n` sender claim needs an explicit anonymity game and priors. Deterministic client selection, amount visibility, age/factor cohorts and known outputs all affect the observable anonymity set. Measure the selected release's actual leakage, rather than equating ring size with a guarantee.
- Quantum-resistant recipient privacy does not make CLSAG spend authorization quantum-resistant. “Ephemeral sender privacy” describes a product risk choice, not a proof that historical financial relationships stop being sensitive.
- Public lottery source→award relationships, fee buckets, origin tags and bridge boundaries remain visible under the candidate design. Define which observer learns what, including a compromised ingress, before promising “untraceable” transactions or balances.

### C3. Hybrid target keys do not yet mean hybrid authenticated memos

[`assemble_hybrid`, `decrypt_memo`, `apply_memo_keystream`](../../transaction/clsag/src/lib.rs) currently use a classical-DH memo key and AES-CTR without an AEAD tag. The hybrid stealth target does not upgrade that memo envelope. This is already documented under #1308/#904. The CT codec must bind the binary memo and amount box to the same authenticated context, with native/WASM/bridge interoperability. A signed outer transaction is a separate integrity mechanism and should not be described as memo AEAD.

### C4. Inactive proofs and payout tests are real progress, with remaining integration gates

The [ownership composition experiment](../../scripts/research/ct-demurrage/OWNERSHIP.md) binds actual CLSAG ownership to the same pseudo-output commitments used in combined arithmetic. The [resource capture](../../scripts/research/ct-demurrage/RESOURCES.md) records ordinary-16 generation around **3.17–3.20 seconds**, verification around **302–308 ms**, and a whole-case peak RSS of **286 MiB** on its recorded macOS setup. Those figures exclude a complete wire/parser/chain integration and do not establish browser/Snap or five-second end-to-end feasibility.

Merged #1365's inactive native accepted-spend slice is also meaningful: seven independent source/repeated/nested payout spends passed locally and in hosted execution. It does not activate V2 or supply remote context proofs, all-client scans or a legacy-claim disposition. Keep the [native-wallet design](../design/lottery-v2-native-wallet.md) and #1286 as the scope reference.

### C5. Economic behavior must survive the privacy redesign

The [candidate contract](../design/ct-transaction-contract.md) preserves ratified EpochOrigin but leaves fee/circulation policy decisions open. Maximum ring factors/ages can charge honest spenders for expensive decoys; avoiding those decoys can expose selection cohorts. This is a privacy, affordability and liveness tradeoff, not just a circuit optimization.

The [fee-funded reinvestment experiment](../../scripts/research/ct-reinvestment/RESULTS.md) found **zero successful award-only consolidations in its main grid**: its proposed reference policy made 0.1-BTH awards cost 0.25 BTH per input to consolidate. This is a bounded candidate-model result, not a universal impossibility claim or a measurement of all live fees. #1372 compares reserve/winner policies. Demonstrate economically usable awards under the accepted policy; do not choose an artificially subsidized demo that conceals the denominator.

### C6. Publishing and performance claims need one release source of truth

The two checked-in PDFs are identical (SHA256 `da42994738b48f6fd093f541f93fc45995362ff16744d37dddc47b482f955060`) and last changed in **July**, while the LaTeX CT implementation-status section changed in September. Extracted PDF text lacks that newer status paragraph. The public PDF URL returned HTTP 403 to the direct fetch used here, so this review does not assert the deployed PDF's hash.

Regenerate both artifacts from reviewed source, validate the download in a browser, and bind its hash to the release manifest. Audit abstract/diagrams/FAQ/landing-page/localized docs together for confidentiality, hybrid derivation, lottery fairness and latency. Replace schematic `~4 KB`, tiny example fees and blanket `<5s` claims with measured, explicitly scoped values for the complete candidate transaction and deployed schedule. Source currently describes a five-second monetary reference; the observed nodes reported a 20-second effective slot duration.

The [threat model](../security/threat-model.md), whitepaper and client trust prompts should also agree about remote-node observation. Dandelion/relay protection after ingress does not erase what that first RPC service already learned.

## 6. Optional bridge lane and explicit exclusions

If the bridge is shown, require BTH→Sepolia and BTH→Solana devnet deposit/mint/burn/release loops with transaction links, separate federation stores, threshold authorization, exactly-once handling and reconciled reserves/combined supply. Existing work is #868/#865/#816; operator prerequisites are #1051, #1052 and #1086. Local Squads execution/recovery evidence is substantial but does not prove live authority configuration or a full BTH round trip.

CT also changes reserve disclosure and encrypted order-memo consumption. Include the final codec and a reviewed reserve-view/attestation mechanism before claiming the bridge proves confidential backing. Mainnet custody and external-audit requirements remain #1019/#830, distinct from test-only demonstrations.

Managed hosting/Stripe (#721), external legal work (#722), the Hyperliquid gas auction (#1048), and commissioning an external audit are not prerequisites for the core testnet showcase. This review does not initiate those activities. Internal review of the new cryptographic composition is a showcase gate; an external assessment remains a later requirement under the existing mainnet plan.

## 7. Evidence and closure rules

- **Performed here:** source/issue review at the pinned baseline; five public status probes plus seed/faucet repeat and faucet-status read; fresh Chrome page loads; web-wallet typecheck; 40 selected Vitest assertions; PDF hash/text/source-date comparison. See [evidence manifest](testnet-showcase-evidence/2026-09-21.json).
- **Existing evidence inspected:** scoped Snap tests/audit, local fullstack harness, merged inactive CT/lottery research and their published limitations. The full Snap suite, fullstack financial test, mobile tests and CT benchmarks were not rerun for this review.
- **Not established:** a successful current live payment, a public Snap installation/send, integrated CT transactions, production LotteryV2, chain recovery under the candidate, or security of the complete composition. Browser pages loading and green mocked tests cannot close these gates.
- **Acceptance package:** exact source/artifact/genesis identity; repeatable commands and environment; accepted transaction/block references; client balance/fee reconciliation; privacy/RPC transcript review; negative tests; bounded performance results; recoverability; and an independent review of the final changes. Redact seeds, private keys and bearer links from all evidence.

Mark this review complete when its findings and task map are accepted. Mark the **full showcase** ready only after every applicable P0 and the rehearsed user journey have evidence on the same release candidate.
