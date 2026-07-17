# Botho MetaMask Snap — Phase-0 Feasibility Spike (issue #815)

## Verdict: **GO**

`bth-wasm-signer` — the node-identical client-side Botho transaction pipeline
(stealth-address scan, ring construction, hybrid ML-KEM-768 outputs, CLSAG
sign, bincode wire format) — **runs correctly and fast inside the MetaMask
Snaps execution environment** (SES / Hardened JavaScript). A Snap prototype
derived a Botho wallet from MetaMask-managed entropy, was funded on a real
testnet chain, **built + CLSAG-signed + submitted a real transaction from
inside the SES sandbox, and the node mined it**.

## Measured send latency (deliverable 1)

Measured **inside the SES executor** (`Date.now()` around each wasm call),
snap bundle running under the official `@metamask/snaps-jest` 10.2 harness.

| Stage (inside SES sandbox) | Time |
|---|---|
| `buildAndSign` — ring construction (ring size 20), 2× hybrid ML-KEM-768 encapsulation, CLSAG sign **+ node-identical self-verification** | **~26–28 ms** |
| `scanOwnedOutputs` (~25 chain outputs, incl. ML-KEM decapsulation attempts) | ~13–15 ms |
| `computeOwnedOutputKeyImages` | ~1 ms |
| Full client pipeline (`buildSendTransaction`: height + outputs RPC fetch → scan → spent-filter → sign) | **~60 ms** |
| `tx_submit` round trip (localhost node) | ~10 ms |

Wall clock per snap invocation from the test harness was ~2.7 s, dominated by
snaps-jest install/IPC overhead, not crypto. Sign latency is **2–3 orders of
magnitude below** any UX threshold — latency is a non-issue.

**Exact measurement environment** (as required by the spike's acceptance
criteria): the snap bundle (wasm inlined) executed by
`@metamask/snaps-execution-environments` 11.2.0 **node-thread executor** — the
real SES lockdown environment — driven by `@metamask/snaps-jest` 10.2.0 /
`@metamask/snaps-simulation`, Node 26.5, Apple Silicon (darwin/arm64). This is
the accepted proxy for a headless MetaMask instance; a real extension runs the
same SES executor in an iframe, so crypto-bound numbers should transfer
(browser wasm JIT differences are small constants).

> **Range proofs**: the live protocol uses PUBLIC (transparent) amounts — there
> are no range proofs today (confidential amounts are a ratified *target*, ADR
> 0006). The measured number therefore covers ring construction + CLSAG + PQ
> encapsulation. If/when Pedersen-committed amounts land, bulletproof
> generation must be re-measured (expected tens of ms in wasm — within budget).

## Entropy → key derivation (deliverable 2)

```
MetaMask SRP
  ── SIP-6 snap_getEntropy (version 1, salt "botho-root")──▶  32-byte entropy
  ── BIP39 entropyToMnemonic (english) ─────────────────────▶  24-word mnemonic
  ── BIP39 seed (empty passphrase) ─────────────────────────▶  64-byte seed
  ── SLIP-10 ed25519 m/44'/866'/0' + HKDF domain separation ▶  Ristretto view/spend keys
  ── node-identical derive_pq_keys_from_seed (wasm) ────────▶  ML-KEM-768 / ML-DSA-65
  ──────────────────────────────────────────────────────────▶  botho://2/ address
```

MetaMask entropy is plugged in **as the BIP39 entropy of the existing
`RootIdentity` pipeline** — nothing downstream changes, and the derived 24-word
mnemonic imported into the web wallet / node recovers the identical wallet.
Verified deterministic across snap re-installs under the same SRP.

**Security trade-off (SRP-derived — chosen — vs Snap-local seed):**

- **SRP-derived (`snap_getEntropy`)** — chosen for the MVP.
  - ✅ One backup: the user's existing MetaMask Secret Recovery Phrase also
    recovers the Botho wallet. No new seed ceremony (matches the low-friction
    product goal).
  - ✅ MetaMask never exposes the SRP itself to the snap — only entropy derived
    from (SRP, **snap id**, salt).
  - ⚠️ SRP compromise now also compromises the Botho wallet (one secret, two
    chains).
  - ⚠️ **Snap-id pinning**: the entropy is bound to the snap's npm id.
    Republishing under a different package name derives a *different* wallet —
    the production snap id must be treated as consensus-critical config, and a
    recovery path (show the derived 24-word Botho mnemonic for off-MetaMask
    backup) should ship in Phase 1.
- **Snap-local independent seed (`snap_manageState`, encrypted at rest)**:
  - ✅ Clean isolation from the SRP.
  - ❌ Needs its own backup/restore UX; losing snap state (uninstall) without a
    backup loses funds. Duplicates exactly the seed-management burden the Snap
    is meant to remove.

## Working testnet send (deliverable 3)

The gated E2E (`BOTHO_SNAP_E2E=1`) spins up a throwaway solo-minting `botho`
node (the same harness as the wasm-signer live tests), funds the snap's
SRP-derived address on-chain, then the **snap** builds, signs, and submits a
1 BTH send back — asserted **accepted and mined** (`tx_get` → confirmed).
Exercised through the snaps-jest harness against a real node RPC ingress
(localhost; the public testnet ingress presents the same JSON-RPC surface, but
has no funded throwaway wallet to drive from CI).

## `getrandom` in the Snap runtime (deliverable 4)

- `crypto.getRandomValues` is endowed in the SES executor (probed: present,
  functional, non-repeating, non-zero).
- All three `getrandom` major versions linked into the wasm (0.2 `js`, 0.3/0.4
  `wasm_js`) resolve at runtime: transaction building **inside the sandbox**
  succeeds, and repeated builds over identical wallet state produce *distinct*
  signed transactions (fresh CLSAG nonces / ring shuffles / KEM encapsulation
  randomness), while each passes the node-identical verifier.

## Bundling

- `wasm-pack build --target bundler` (`pnpm --filter @botho/wasm-signer
  build:wasm:bundler` → `pkg-bundler/`, git-ignored) pairs exactly with
  snaps-cli's `experimental.wasm` webpack loader, which base64-inlines the
  wasm and resolves its JS-glue imports. Bundle: **~600 KB** (wasm is ~360 KB).
- Permissions: `endowment:webassembly`, `endowment:network-access` (node RPC),
  `snap_getEntropy`, `endowment:rpc`.
- `mm-snap eval` (SES compatibility check) passes.

## What the spike found and fixed (in this PR)

1. **`bth-wasm-signer` could not spend hybrid (6.0.0) outputs** — the sign path
   hardcoded classical one-time-key recovery, silently derived the wrong key
   for any post-#978 output (i.e. *every* received output, including solo
   coinbases) and failed CLSAG self-verification. Fixed by threading the wallet
   seed + per-input `output_index`/`kem_ciphertext` into `SignRequest` and
   using the same unified `recover_spend_key_for` the key-image path uses
   (native regression test added). This also unblocks the web wallet's send
   path for hybrid-funded wallets.
2. **`node-harness.ts` never mined post-#770**: an explicit-mode solo node is
   seeded `initial_sync_complete = false` and, with zero peers, stays in sync
   Discovery forever, so minting never arms. The harness now uses
   `mode = "recommended"` + `min_peers = 0` (the solo carve-out).
3. **`send-live-node.test.ts` drift**: the injected test signer lacked the
   address-derivation surface, and its output mapping/scans predate hybrid
   outputs. Fixed; the gated #372 E2E passes again.

## Running the spike

```sh
# 1. wasm artifacts (both targets)
pnpm --filter @botho/wasm-signer build:wasm build:wasm:bundler

# 2. environment probes only (no node needed)
pnpm --filter @botho/snap-spike test:snap

# 3. full E2E with a real chain (needs the node binary)
cargo build --release --bin botho
BOTHO_SNAP_E2E=1 BOTHO_BIN=$PWD/target/release/botho \
  pnpm --filter @botho/snap-spike test:snap
```

## Phase-1 notes (from the issue's open questions)

- **Scanning cost** is the real scaling risk, not signing: the spike (like the
  web wallet) fetches and scans the full output set per operation. Fine at
  harness scale; the live testnet already has tens of thousands of outputs.
  Phase 1 needs incremental/windowed scanning with persisted scan state
  (`snap_manageState`) — same problem the web wallet has, so solve it in the
  shared package.
- **Node trust / wrong-network guard**: carry over the web wallet's ingress
  model (#811) for user-selected RPC endpoints.
- The `senderMnemonic`/`botho_deriveAddress` RPC methods are **spike-only test
  hooks** (they let the harness drive the funding leg through the snap). A real
  snap must never accept caller-supplied key material — strip them in Phase 1.
- Distribution: npm publish + MetaMask allowlist; pin the snap id (see
  derivation trade-off above).
