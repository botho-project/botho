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
  code). Exposed as functions the webhook calls — there is **no public
  `/provision` route**.
- **P7.2 — the billing↔provisioning join** (#458 §2 + §5, issue #506): a
  `/webhook` endpoint that **HMAC-verifies the Stripe signature over the raw
  body** (with a timestamp tolerance to defeat replay) and only then maps the
  event to `provisionRig` (`checkout.session.completed` / `invoice.paid`) or
  `teardownRig` (`customer.subscription.deleted` / `invoice.payment_failed`).
  Idempotent against Stripe's retries (the provisioner dedups by
  `subscription_id`); unknown event types are a 2xx no-op.

> Out of scope here (later phases of #458 §8):
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
| POST   | `/webhook`  | Stripe-signed webhook → provision / deprovision (#506).      |
| GET    | `/healthz`  | Liveness probe.                                              |

There is **no `/provision` route** — a managed rig can only be launched via the
signature-verified `/webhook` (#458 §5).

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

### `POST /webhook` (#506 — the billing↔provisioning join)

The **only** path that can launch or tear down a rig. Stripe POSTs the event
JSON with a `Stripe-Signature` header; the handler:

1. **Reads the RAW body** (never JSON-parses before verifying — the HMAC is over
   the exact bytes Stripe signed, and parsing unverified input is avoided).
2. **Verifies the signature**: `Stripe-Signature` is `t=<ts>,v1=<hmac>`; the
   handler recomputes `HMAC-SHA256(STRIPE_WEBHOOK_SECRET, "<ts>.<rawBody>")` and
   constant-time compares it. A **5-minute timestamp tolerance** rejects stale
   deliveries (replay defense — #458 §5). Unsigned / mismatched / stale → **400**
   with **no side effect** (no parse, no provisioner call).
3. **Maps the verified event** to an action and ACKs Stripe quickly with `2xx`:

   | Stripe event                     | Action       | Provisioner call             |
   |----------------------------------|--------------|------------------------------|
   | `checkout.session.completed`     | provision    | `provisionRig(req, deps)`    |
   | `invoice.paid`                   | provision    | `provisionRig(req, deps)`    |
   | `customer.subscription.deleted`  | teardown     | `teardownRig(subId, deps)`   |
   | `invoice.payment_failed`         | teardown     | `teardownRig(subId, deps)`   |
   | anything else                    | no-op (2xx)  | —                            |

   The provision request is read from the event: `subscription` + `customer` +
   `metadata.region` (the region captured at checkout, P7.1 — also read from
   `subscription_details.metadata` / line-item metadata for invoice events). For
   `customer.subscription.deleted` the event object *is* the subscription, so its
   `id` is the subscription id.

4. **Idempotency:** Stripe retries deliveries; a replay flows back through
   `provisionRig`/`teardownRig`, which dedup by `subscription_id` (D1 + the EC2
   tag), so a duplicate delivery **never launches a second instance**.

**Responses:** `200 {received, action}` on success (including no-op events),
`400` bad/missing/stale signature or unparseable body, `405` non-POST, `500`
when `STRIPE_WEBHOOK_SECRET` or the provisioner env is unconfigured (fail closed
so Stripe retries once configured rather than acting unverified).

A failed provision/teardown is logged but still `2xx`-acked (the provisioner is
idempotent; the SEC reconciliation cron, #508, is the safety net) so Stripe does
not hammer the endpoint.

#### Local webhook testing

```bash
stripe listen --forward-to localhost:8787/webhook   # prints a whsec_ for .dev.vars
stripe trigger checkout.session.completed
```

## Configuration (secrets & vars)

All secrets come from Worker secrets / vars — **never the repo** (#458 §2, §5).

| Key                    | Kind   | Notes                                                        |
|------------------------|--------|-------------------------------------------------------------|
| `STRIPE_SECRET_KEY`    | secret | TEST key (`sk_test_...`) while on testnet (#458 §7).        |
| `STRIPE_PRICE_ID`      | secret | The recurring $50/mo Price id (`price_...`). See below.     |
| `STRIPE_WEBHOOK_SECRET`| secret | Webhook signing secret (`whsec_...`). Required for `/webhook`.|
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
   wrangler secret put STRIPE_SECRET_KEY     # paste sk_test_...
   wrangler secret put STRIPE_PRICE_ID       # paste price_...
   ```

4. **Webhook:** create a webhook endpoint pointing at the deployed Worker's
   `/webhook` URL, subscribed to `checkout.session.completed`, `invoice.paid`,
   `customer.subscription.deleted`, `invoice.payment_failed`. Copy its signing
   secret (`whsec_...`):

   ```bash
   wrangler secret put STRIPE_WEBHOOK_SECRET # paste whsec_...
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

`src/webhook.test.ts` and `src/webhook-handler.test.ts` cover the `/webhook`
signature crypto (valid / tampered / wrong-secret / stale / missing), the
event→action mapping, raw-body verification, idempotent replay (no double
launch), teardown, and the unknown-event no-op — all with in-memory provisioner
fakes (no network, no live Stripe/AWS/DNS/D1).

## Deploy

```bash
pnpm --filter @botho/baas-worker deploy   # wrangler deploy
```
