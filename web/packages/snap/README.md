# Botho MetaMask Snap — Phase-1 MVP (issue #815)

Manage a privacy-by-default **Botho** wallet from inside MetaMask, without any
EVM compromise. The Snap derives Botho keys from the MetaMask Secret Recovery
Phrase, runs the node-identical `bth-wasm-signer` inside the MetaMask Snaps SES
sandbox, and talks to a user-selected Botho node for receive / balance / send.

This is the **Phase-1 MVP** promotion of the Phase-0 feasibility spike
(`web/packages/snap-spike`, verdict **GO**, merged as PR #1055). The spike proved
the hard parts — wasm-in-SES, `getrandom` under the sandbox, SIP-6 key
derivation, and a real testnet send end-to-end (~26–28 ms `buildAndSign`). This
package turns that into a clean, self-contained MVP: no spike-only test hooks, a
real RPC surface with dialogs, and a green mocked-node test suite.

> The Snap is a **pure client**. It makes **no change** to the Botho protocol,
> consensus, or wire format — it is a second consumer of the already-audited
> `@botho/wasm-signer` core, exactly like the web wallet.

## RPC methods

All params/results are JSON-safe; amounts are string-encoded `u64` picocredits
(1 BTH = 10¹² picocredits).

| Method | Params | Dialog | Result |
|---|---|---|---|
| `botho_getAddress` | — | none | `{ address, derivation }` |
| `botho_getBalance` | `{ rpcUrl }` | none | `{ spendablePicocredits }` |
| `botho_send` | `{ rpcUrl, recipientAddress, amountPicocredits, feePicocredits? }` | confirmation | `{ txHash, txBytes }` |
| `botho_showReceive` | — | alert (address) | `{ address }` |
| `botho_showBalance` | `{ rpcUrl }` | alert (balance) | `{ spendablePicocredits }` |
| `botho_showMnemonic` | — | confirm → alert | `{ revealed }` |

`rpcUrl` is the **user-selected ingress node**, carrying over the web wallet's
node-trust model. Before any balance/send the Snap runs a wrong-network guard
(`node_getStatus.network` must equal `botho-testnet`; loopback hosts are exempt),
mirroring the web wallet's `validateRpcEndpointForNetwork` (#811). Keys **never
leave the sandbox**.

## Key derivation: MetaMask SRP → Botho RootIdentity

```
MetaMask SRP
  ── SIP-6 snap_getEntropy (version 1, salt "botho-root") ──▶  32-byte entropy
  ── BIP39 entropyToMnemonic (english) ────────────────────▶  24-word mnemonic
  ── BIP39 seed (empty passphrase) ────────────────────────▶  64-byte seed
  ── SLIP-10 ed25519 m/44'/866'/0' + HKDF domain separation▶  Ristretto view/spend keys
  ── node-identical derive_pq_keys_from_seed (wasm) ───────▶  ML-KEM-768 / ML-DSA-65
  ─────────────────────────────────────────────────────────▶  tbotho://2/ address
```

MetaMask entropy is plugged in **as the BIP39 entropy of the existing
`RootIdentity` pipeline** (`@botho/core` `deriveKeypairs`), so nothing downstream
changes and the derived 24-word mnemonic imported into the web wallet / CLI /
mobile wallet recovers the **identical** wallet. `botho_showMnemonic` reveals
those words (behind an explicit confirmation) so the wallet is recoverable
off-MetaMask.

### Security trade-off: SRP-derived (chosen) vs Snap-local seed

**SRP-derived (`snap_getEntropy`)** — chosen for the MVP:

- ✅ **One backup**: the user's existing MetaMask Secret Recovery Phrase also
  recovers the Botho wallet. No new seed ceremony — matches the low-friction
  product goal.
- ✅ MetaMask never exposes the SRP itself to the Snap — only entropy derived
  from `(SRP, snap id, salt)`.
- ⚠️ **Coupled secrets**: an SRP compromise now also compromises the Botho
  wallet (one secret, two chains).
- ⚠️ **Snap-id pinning**: the entropy is bound to this Snap's npm id.
  Republishing under a different package name derives a *different* wallet, so
  the production snap id must be treated as consensus-critical config. Mitigation
  shipped here: `botho_showMnemonic` exports the derived 24-word phrase for an
  off-MetaMask backup.

**Snap-local independent seed (`snap_manageState`, encrypted at rest)** — the
alternative:

- ✅ Clean isolation from the SRP.
- ❌ Needs its own backup/restore UX; losing Snap state (uninstall) without a
  backup loses funds — reintroducing exactly the seed-management burden the Snap
  is meant to remove.

## Architecture

| File | Responsibility |
|---|---|
| `src/index.ts` | `onRpcRequest` handler + dialog orchestration |
| `src/derivation.ts` | SIP-6 entropy → Botho `SnapWallet` (SLIP-10 + PQ keys + address) |
| `src/signer.ts` | Inject the inlined bundler wasm into `@botho/wasm-signer` via `setSigner` |
| `src/node.ts` | JSON-RPC node client, `isValidRpcUrl`, wrong-network guard, `SendRpc` |
| `src/ui.ts` | Snaps custom-UI dialog content (receive / balance / send / mnemonic) |
| `src/format.ts` | SES-safe (Intl-free) picocredit → BTH formatting |

The wasm is `wasm-pack build --target bundler` output, base64-inlined into the
bundle by snaps-cli's `experimental.wasm` loader (`snap.config.ts`) — the exact
pattern the spike validated.

## Building & testing

```sh
# 1. wasm artifact (git-ignored; produces packages/wasm-signer/pkg-bundler)
pnpm --filter @botho/wasm-signer build:wasm:bundler

# 2. build the snap bundle (dist/bundle.js) + refresh the manifest shasum
pnpm --filter @botho/snap build:snap

# 3. run the test suite (builds, then runs snaps-jest under the real SES executor)
pnpm --filter @botho/snap test:snap
```

Tests run through the official `@metamask/snaps-jest` / `@metamask/snaps-simulation`
node-thread SES executor — the accepted headless proxy for a real MetaMask
instance (validated in the spike). The node RPC is **mocked** with an in-process
JSON-RPC server (`test/mock-node.ts`); **no live betanet or `botho` binary is
required**. Tests are named `*.snap.ts` (not `*.test.ts`) so the workspace-root
vitest run never picks them up.

Coverage:

- `derivation.snap.ts` — SIP-6 derivation determinism + valid `tbotho://2/` address.
- `balance.snap.ts` — `getBalance` against a mocked ingress (fresh wallet → 0),
  receive/balance dialogs, malformed-URL rejection.
- `send.snap.ts` — send confirmation dialog, user approve/reject branches,
  bad-recipient / bad-amount guards, and that the pipeline reaches the node and
  never submits without funds.
- `units.snap.ts` — pure-logic tests for the wrong-network guard (incl. the
  loopback exemption a loopback mock can't trigger) and SES-safe formatting.

## Deferred (out of scope for this MVP)

- **Live-testnet send validation** — a real on-chain send from the Snap needs
  real owned-output fixtures that only a live minting node can produce, and the
  public betanet is currently **frozen** (height 202, minting paused — #1051).
  The send *plumbing* is fully tested against a mocked node here; end-to-end
  live-send validation is a follow-up, **blocked on betanet resume (#1051)**.
  (The spike demonstrated a working real send against a throwaway solo node; that
  path relied on spike-only test hooks that are intentionally absent here.)
- **Phase 2 parity** (per the issue): transaction history, contacts, in-Snap
  ingress selection UX, i18n, incremental/windowed scanning with persisted scan
  state (`snap_manageState` — the shared scaling concern with the web wallet),
  and a dedicated **security pass** for the new key-handling surface (cf. the
  web-wallet at-rest audit, #474/#475).
- **Publishing / distribution** — npm publish + MetaMask allowlist, and pinning
  the production snap id (see the snap-id pinning trade-off above).

## Non-goals

No EVM / `eth_*` compatibility, no secp256k1 transparent txs, no bridge, and no
change to the Botho protocol, consensus, or wire format. The Snap is a pure
client.
