# @botho/baas-worker

Cloudflare Worker for the **Botho-as-a-Service** control plane (parent #458 §1).

This package implements:

- **P7.1 — the billing front door** (#458 §2, issue #504): a `/checkout` endpoint
  that creates a Stripe Checkout Session for a single **$50/mo subscription**.
- **P6.2 — the provisioner core** (#458 §3 + §5, issue #502): `provisionRig()` /
  `teardownRig()` — given a `subscription_id` they launch (or reconcile) a
  per-subscription Botho rig (EC2 `RunInstances` + Cloudflare DNS + a D1
  mapping), **idempotent by `subscription_id`** and **safe by construction**
  (region/instance-type allowlists, per-sub cap, global fleet cap enforced in
  code). Exposed as functions for the webhook to call — there is **no public
  `/provision` route**.

> Out of scope here (later phases of #458 §8):
> - `/webhook` (Stripe signature verify → calls `provisionRig`/`teardownRig`) — **P7.2 (#506)**
> - `/status` (user looks up their rig) — **P6.3**
> - tighter provisioner IAM policy + the orphan-reconciliation cron — **SEC (#508)**
>
> The frontend "Get a rig" surface that calls `/checkout` lives in
> `@botho/web-wallet` at `/rig` (route `RigPage`).

## Provisioner core (P6.2 / #502)

`provisionRig(req, deps)` is the control-plane flow. `req` is
`{ subscriptionId, customerId, region, instanceType?, rigId? }`; `deps` are the
injectable `ec2` / `dns` / `store` clients (built from the env via
`depsFromEnv(env)`). It:

1. **Fails closed on the safety caps** (#458 §5) BEFORE any AWS call:
   region must be in `REGION_ALLOWLIST` (start: `us-west-2`), instance type is
   forced to `t4g.medium` (off-list types rejected), and the **global fleet cap**
   (`FLEET_CAP`, default 25) is checked against the count of active D1 rows.
2. **Idempotency by `subscription_id`** (#458 §3, §5): checks D1 first, then
   reconciles against the EC2 `botho:subscription` tag, so a replayed trigger
   **never launches a second instance** (it adopts an existing/orphaned one).
3. Launches EC2 `RunInstances` with the proven compute shape, tagged
   `botho:managed-rig=true` / `botho:subscription=<id>` / `botho:user=<id>` /
   `botho:rig-id=<id>`, and **user-data** = a small script that fetches and runs
   `infra/baas/rig-bootstrap.sh` (#499/#521) with `RIG_ID`/`REGION`/`TIER`.
4. Creates the Cloudflare DNS `A rig-<id>.testnet.botho.io -> <public IP>`.
5. Writes/advances the D1 row (`provisioning → running`; `suspended`/`terminated`
   reserved for teardown).

`teardownRig(subscriptionId, deps)` terminates the instance, deletes the DNS
record, and marks the D1 row `terminated` (idempotent; callable by SEC/P7.2).

The module boundary is fully mockable: EC2 (`Ec2Client`), DNS (`DnsClient`), and
D1 (`RigStore`) are interfaces, so every test uses in-memory fakes and **no real
network/AWS/Cloudflare call ever runs in a test code path**.

### D1 schema

`schema.sql` defines the `rigs` table (`subscription_id` UNIQUE = the idempotency
anchor). Apply it before first use:

```bash
wrangler d1 create botho-baas          # copy the id into wrangler.toml
wrangler d1 execute botho-baas --remote --file=schema.sql
```

### Provisioner secrets (Worker secrets — never the repo)

A **dedicated, tightly-scoped provisioner IAM user** (NOT `botho-deploy`; the
IAM-policy hardening is SEC/#508):

```bash
wrangler secret put AWS_ACCESS_KEY_ID
wrangler secret put AWS_SECRET_ACCESS_KEY
wrangler secret put CF_DNS_API_TOKEN     # Cloudflare Zone:DNS:Edit token
wrangler secret put CF_DNS_ZONE_ID       # testnet.botho.io zone id
# optional:
wrangler secret put BOTHO_BINARY_URL     # linux-aarch64 botho binary the rig downloads
wrangler secret put BOTHO_BINARY_SHA256
```

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
