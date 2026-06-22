# @botho/baas-worker

Cloudflare Worker for the **Botho-as-a-Service** control plane (parent #458 §1).

This package currently implements **P7.1 — the billing front door** (#458 §2,
issue #504): a `/checkout` endpoint that creates a Stripe Checkout Session for a
single **$50/mo subscription**, plus the env/secret contract for running it in
Stripe **TEST mode** while on testnet (#458 §7).

> Out of scope here (later phases of #458 §8):
> - `/webhook` (Stripe signature verify → provision/deprovision) — **P7.2 (#506)**
> - `/status` (user looks up their rig) — **P6.3**
>
> The frontend "Get a rig" surface that calls `/checkout` lives in
> `@botho/web-wallet` at `/rig` (route `RigPage`).

## Endpoints

| Method | Path        | Purpose                                                       |
|--------|-------------|--------------------------------------------------------------|
| POST   | `/checkout` | Create a `mode=subscription` Stripe Checkout Session.        |
| GET    | `/healthz`  | Liveness probe.                                              |

### `POST /checkout`

**Request body** (JSON):

```json
{ "region": "us-west-2", "email": "optional@example.com" }
```

- `region` (required) — desired AWS region for the rig. Must be in the
  server-side allowlist (`REGION_ALLOWLIST`, starts as `["us-west-2"]`, #458 §5).
  Re-validated server-side so a crafted request can never request an off-list
  region.
- `email` (optional) — pre-fills the Stripe checkout email.

**Success response** (`200`):

```json
{ "id": "cs_test_...", "url": "https://checkout.stripe.com/c/..." }
```

The frontend redirects the browser to `url`.

**Errors:** `400` invalid body / region, `405` non-POST, `500` worker not
configured (fails closed — never calls Stripe with an empty key), `502` Stripe
rejected the request.

The created session carries:
- `mode=subscription`, one line item = the `$50/mo` Price (`STRIPE_PRICE_ID`),
  quantity 1 (one rig per subscription).
- `metadata.region` **and** `subscription_data.metadata.region` so the webhook
  (P7.2) can read the region from either the session or the subscription.
- `success_url` with Stripe's `{CHECKOUT_SESSION_ID}` template appended for the
  future status lookup (P6.3).

## Configuration (secrets & vars)

All secrets come from Worker secrets / vars — **never the repo** (#458 §2, §5).

| Key                    | Kind   | Notes                                                        |
|------------------------|--------|-------------------------------------------------------------|
| `STRIPE_SECRET_KEY`    | secret | TEST key (`sk_test_...`) while on testnet (#458 §7).        |
| `STRIPE_PRICE_ID`      | secret | The recurring $50/mo Price id (`price_...`). See below.     |
| `CHECKOUT_SUCCESS_URL` | var    | Stripe success redirect (in `wrangler.toml`).               |
| `CHECKOUT_CANCEL_URL`  | var    | Stripe cancel redirect (in `wrangler.toml`).                |
| `ALLOWED_ORIGINS`      | var    | Comma-separated browser origins allowed to call `/checkout`.|

### Stripe setup (one-time, TEST mode)

In the Stripe dashboard (legal entity **2amlogic**, **test mode** while on
testnet):

1. **Product:** create "Botho Managed Rig (testnet)".
2. **Price:** add a **recurring price = $50.00 / month** (USD). Copy its id
   (`price_...`).
3. Store the secret key and price id in the Worker:

   ```bash
   wrangler secret put STRIPE_SECRET_KEY   # paste sk_test_...
   wrangler secret put STRIPE_PRICE_ID     # paste price_...
   ```

Flip to LIVE only when the maintainer decides (re-run the steps with live-mode
values). The §6/§7 economics must be settled before charging real money.

### Local development

```bash
cp .dev.vars.example .dev.vars   # fill in your Stripe TEST values
pnpm --filter @botho/baas-worker dev
```

`.dev.vars` is gitignored. Never commit real keys.

## Tests

```bash
# from web/
pnpm test:run
```

`src/checkout.test.ts` and `src/index.test.ts` cover the session-param builder,
input validation, the region allowlist, fail-closed config handling, and the
`/checkout` handler — all with a mocked `fetch` (no network, no live Stripe).

## Deploy

```bash
pnpm --filter @botho/baas-worker deploy   # wrangler deploy
```
