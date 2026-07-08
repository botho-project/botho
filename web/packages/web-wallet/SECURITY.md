# Botho Web Wallet — Security & Threat Model

This document states honestly what the Botho web wallet (`botho.io`)
does and does not protect, so users can make an informed decision about how to
hold value in it. It reflects the code as shipped, not aspirations.

## Trust model: non-custodial, keys in your browser

The web wallet is **non-custodial**. Your seed mnemonic and all derived keys are
generated, stored, and used **only in your browser**. Nothing secret is ever
sent to a server — transactions are built and signed locally (in a WebAssembly
signer, `@botho/wasm-signer`) and only the finished, signed transaction is
submitted to a node. There is no account, no server-side key escrow, and no
password-reset: if you lose your seed and your device, the funds are gone.

## What IS protected at rest

All sensitive data is encrypted at rest under a single **password-derived vault
key** (PBKDF2-SHA256, 600k iterations → AES-256-GCM; see
`@botho/core` `wallet/vault.ts`). The key is derived from your wallet password
when you unlock and is held only in memory for the session. Encrypted at rest:

| Data | Issue | Storage key |
|------|-------|-------------|
| Seed mnemonic | #475 | `botho-wallet-encrypted` blob |
| Claim-link bearer secrets (ephemeral mnemonics for unclaimed payment links) | #474 | `botho-claim-links` |
| Address book (contact names, addresses, notes, tx counts — your counterparty graph) | #476 | `botho-address-book` |

Each is stored as a self-describing, versioned vault blob. Legacy plaintext data
written by older versions is transparently re-encrypted ("re-wrapped") the first
time the wallet is unlocked. For a password-protected wallet, **none of the
above is ever written to `localStorage` in cleartext.**

### Plaintext / no-password wallets

If you create a wallet **without a password**, there is no vault key, so:

- The seed is the one thing that must persist; older builds stored it without a
  password. Prefer setting a password.
- Claim-link bearer secrets are **not** persisted without a password (sending via
  a claim link requires a password, and fails fast *before* any on-chain spend
  so funds can never be stranded).
- The address book is **not persisted** without a password and is unavailable
  until you add one — the contact graph is never written in plaintext. Saving a
  contact simply does nothing durable until the wallet has a password.

**Recommendation: always set a wallet password.**

## What is NOT protected

Encryption-at-rest does not defend against an attacker who can run code on the
origin or read process memory **while the wallet is unlocked**:

- **XSS / malicious dependency.** Any JavaScript running on `botho.io`
  can read `localStorage` and, once you have unlocked, the decrypted seed and
  vault key live in memory and can be read by that code. The at-rest encryption
  protects a *locked* wallet (cold storage, shared device, another extension
  reading disk), not a live, unlocked tab. We reduce this surface with a strict
  CSP (below) and by minimizing third-party JavaScript, but cannot eliminate it.
- **Malicious browser extension.** An extension with access to the page can read
  page memory and storage just like XSS. Use a dedicated browser profile with no
  untrusted extensions for anything valuable.
- **The node sees your queries (thin-client privacy tradeoff).** The wallet is a
  thin client: it asks a node about your addresses / UTXOs / key images. That
  node therefore learns which addresses and outputs you are interested in and
  your IP. This is acceptable *only for a node you trust*; it is the standard
  thin-client privacy cost. Botho's on-chain privacy (ring signatures,
  stealth addresses) still protects observers of the chain itself, but your
  chosen ingress node sees your interest. Run your own node for maximum privacy.
- **Phishing / fake sites.** Always confirm the origin is `botho.io`.

## Hardening shipped

### Content-Security-Policy (#476)

`public/_headers` ships a strict CSP on `botho.io` (Cloudflare Pages):

```
default-src 'self';
script-src 'self' 'wasm-unsafe-eval';
style-src 'self' 'unsafe-inline';
img-src 'self' data:;
font-src 'self' data:;
connect-src 'self' https:;
worker-src 'self' blob:;
manifest-src 'self';
object-src 'none';
base-uri 'self';
frame-ancestors 'none';
form-action 'self';
upgrade-insecure-requests
```

Key points:

- **No `'unsafe-inline'` / `'unsafe-eval'` in `script-src`.** The build emits
  only external scripts, and the PWA service-worker registration runs from a
  bundled module (not an inline script). `'wasm-unsafe-eval'` is required solely
  to instantiate the WebAssembly signer; it does **not** permit `eval`/inline JS.
- **`connect-src 'self' https:` permits any HTTPS origin.** This is deliberate:
  the wallet is a thin client that must reach whatever node you point it at,
  including a user-entered **Custom RPC** endpoint or your own / a managed,
  trusted node (`NetworkSelector.tsx` → `createCustomNetwork`). Those origins are
  added at **runtime** and cannot be enumerated in a static header, so a fixed
  allowlist would silently break the Custom RPC feature. `http:` is **not**
  allowed — node RPC is HTTPS-only, and `upgrade-insecure-requests` stays.
  - **Tradeoff:** because connect-src is open to all HTTPS hosts, it is **not**
    an exfiltration backstop — an XSS could POST stolen data to any HTTPS server.
    The primary anti-XSS defenses are therefore `script-src 'self'` + **no**
    `'unsafe-inline'` / `'unsafe-eval'` (an injected attacker cannot execute
    script in the first place), framing locks, and dependency hygiene — not
    `connect-src`.
- **`frame-ancestors 'none'` / `X-Frame-Options: DENY`** prevent clickjacking.

### Subresource integrity / supply chain

- Third-party JavaScript is minimized; the app is built from pinned workspace
  dependencies (`pnpm` lockfile). There are no third-party `<script>` tags in
  `index.html`, so there is nothing cross-origin to pin with SRI today; the CSP
  `script-src 'self'` already forbids loading off-origin scripts. The wasm signer
  is a first-party artifact served from the same origin.

## User guidance

- **Set a wallet password** so everything at rest is encrypted.
- **Use a dedicated browser profile** (no untrusted extensions) for real value.
- **Use a node you trust** as your ingress, or run your own.
- **Back up your seed** offline; there is no recovery without it.

## Roadmap

- Hardware / passkey-backed key storage.
- Optional self-hosted node bundling for full thin-client privacy.
