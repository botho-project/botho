# @botho/baas-worker

Cloudflare Worker for the **Botho-as-a-Service** control plane (parent #458 §1).

This package implements:

- **P7.1 — the billing front door** (#458 §2, issue #504): a `/checkout` endpoint
  that creates a Stripe Checkout Session for a single **$50/mo subscription**.
- **P6.2 — the provisioner core** (#458 §3 + §5, issue #502): `provisionNode()` /
  `teardownNode()` — given a `subscription_id` they launch (or reconcile) a
  per-subscription Botho node (EC2 `RunInstances` + Cloudflare DNS + a D1
  mapping), **idempotent by `subscription_id`** and **safe by construction**
  (region/instance-type allowlists, per-sub cap, global fleet cap enforced in
  code). Exposed as functions the webhook calls — there is **no public
  `/provision` route**.
- **P7.2 — the billing↔provisioning join** (#458 §2 + §5, issue #506): a
  `/webhook` endpoint that **HMAC-verifies the Stripe signature over the raw
  body** (with a timestamp tolerance to defeat replay) and only then maps the
  event to `provisionNode` (`checkout.session.completed` / `invoice.paid`) or
  `teardownNode` (`customer.subscription.deleted` / `invoice.payment_failed`).
  Idempotent against Stripe's retries (the provisioner dedups by
  `subscription_id`); unknown event types are a 2xx no-op.
- **P6.3 — user→node mapping + status lookup** (#458 §3 step 5 + §4 + §6, issue
  #507): a `/status` endpoint that, for an authenticated user (a **magic-link
  status token** that binds to one Stripe customer), returns their node's RPC URL,
  lifecycle state, and a **live health summary from `node_getStatus`**. A
  `/portal` endpoint opens a **Stripe Customer Portal** session so the user can
  manage/cancel the subscription. Both are **authz-scoped**: the customer id only
  ever comes from the verified token, and the D1 lookup is keyed on it, so a user
  can only see their own node. `/status` is read-only — it can never provision.

- **SEC — security hardening** (#458 §5, issue #508): a **dedicated,
  least-privilege provisioner IAM policy** (committed at `iam/provisioner-policy.json`
  + `iam/README.md`), an **orphan-terminating reconciliation cron** (a scheduled
  Worker that lists every `botho:managed-node=true` EC2 instance, cross-checks each
  `botho:subscription` against Stripe, and **terminates orphans** — the cost-bleed
  safety net behind the webhook teardown), and an **explicit per-subscription cap**
  cross-checked against EC2 tags before launch. See "Security hardening" below.

> The frontend "Get a node" surface that calls `/checkout` lives in
> `@botho/web-wallet` at `/node` (route `NodePage`).

## Provisioner core (P6.2 / #502)

`provisionNode(req, deps)` is the control-plane flow. `req` is
`{ subscriptionId, customerId, region, instanceType?, nodeId? }`; `deps` are the
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
   `botho:managed-node=true` / `botho:subscription=<id>` / `botho:user=<id>` /
   `botho:node-id=<id>`, and **user-data** = a small script that fetches and runs
   `infra/baas/node-bootstrap.sh` (#499/#521) with `NODE_ID`/`REGION`/`TIER`.
4. Creates the Cloudflare DNS `A node-<id>.testnet.botho.io -> <public IP>`.
5. Writes/advances the D1 row (`provisioning → running`; `suspended`/`terminated`
   reserved for teardown).

`teardownNode(subscriptionId, deps)` terminates the instance, deletes the DNS
record, and marks the D1 row `terminated` (idempotent; callable by SEC/P7.2).

The module boundary is fully mockable: EC2 (`Ec2Client`), DNS (`DnsClient`), and
D1 (`NodeStore`) are interfaces, so every test uses in-memory fakes and **no real
network/AWS/Cloudflare call ever runs in a test code path**.

### D1 schema

`schema.sql` defines the `nodes` table (`subscription_id` UNIQUE = the idempotency
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
wrangler secret put BOTHO_BINARY_URL     # linux-aarch64 botho binary the node downloads
wrangler secret put BOTHO_BINARY_SHA256
```

## Endpoints

| Method | Path        | Purpose                                                       |
|--------|-------------|--------------------------------------------------------------|
| POST   | `/checkout` | Create a `mode=subscription` Stripe Checkout Session.        |
| POST   | `/webhook`  | Stripe-signed webhook → provision / deprovision (#506).      |
| GET    | `/status`   | Authenticated node lookup: RPC URL + state + health (#507).   |
| POST   | `/portal`   | Open a Stripe Customer Portal session (manage/cancel, #507). |
| GET    | `/healthz`  | Liveness probe.                                              |

There is **no `/provision` route** — a managed node can only be launched via the
signature-verified `/webhook` (#458 §5).

### `POST /checkout`

**Request body** (JSON):

```json
{ "region": "us-west-2", "email": "optional@example.com" }
```

- `region` (required) — desired AWS region for the node. Must be in the
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
  quantity 1 (one node per subscription).
- `metadata.region` **and** `subscription_data.metadata.region` so the webhook
  (P7.2) can read the region from either the session or the subscription.
- `success_url` with Stripe's `{CHECKOUT_SESSION_ID}` template appended for the
  future status lookup (P6.3).

### `POST /webhook` (#506 — the billing↔provisioning join)

The **only** path that can launch or tear down a node. Stripe POSTs the event
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
   | `checkout.session.completed`     | provision    | `provisionNode(req, deps)`    |
   | `invoice.paid`                   | provision    | `provisionNode(req, deps)`    |
   | `customer.subscription.deleted`  | teardown     | `teardownNode(subId, deps)`   |
   | `invoice.payment_failed`         | teardown     | `teardownNode(subId, deps)`   |
   | anything else                    | no-op (2xx)  | —                            |

   The provision request is read from the event: `subscription` + `customer` +
   `metadata.region` (the region captured at checkout, P7.1 — also read from
   `subscription_details.metadata` / line-item metadata for invoice events). For
   `customer.subscription.deleted` the event object *is* the subscription, so its
   `id` is the subscription id.

4. **Idempotency:** Stripe retries deliveries; a replay flows back through
   `provisionNode`/`teardownNode`, which dedup by `subscription_id` (D1 + the EC2
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

### `GET /status` (#507 — the authenticated node lookup)

A returning user looks up their node with a **magic-link status token** — the
MVP identity model (#458 §4): no password, the signed link IS the credential.

```
GET /status?token=<cus_…>.<exp>.<hmac>
```

The token is `<stripeCustomerId>.<expUnixSeconds>.<HMAC-SHA256(STATUS_LINK_SECRET,
"<customerId>.<exp>")>` (minted by `mintStatusToken`). The handler:

1. Verifies the HMAC (constant-time) and the expiry. A tampered customer id or
   forged/expired token → **401** with no data leak (`verifyStatusToken`).
2. Takes the customer id **only from the verified token**, never from the
   request, and looks the node up keyed on that customer (`getByCustomer`). So a
   valid token for customer A can never surface customer B's node.
3. Probes the node's health via `node_getStatus` (only for `running` nodes; a
   `provisioning`/`suspended`/`terminated` node reports `health.status:"unknown"`
   without a node call). A down node never fails the response — it reports
   `"offline"`.

**Success response** (`200`):

```json
{
  "nodeId": "abc123",
  "rpcUrl": "https://node-abc123.testnet.botho.io/rpc",
  "state": "running",
  "region": "us-west-2",
  "health": { "status": "online", "chainHeight": 42, "synced": true },
  "walletDeepLink": "https://wallet.botho.io/wallet?rpc=https%3A%2F%2Fnode-abc123.testnet.botho.io%2Frpc"
}
```

`walletDeepLink` opens the PWA with the node's RPC pre-selected as the "custom
RPC" ingress (#458 §3 step 5).

**Errors:** `400` missing token, `401` invalid/expired/forged token (no leak),
`404` token valid but the customer has no node, `405` non-GET, `500` not
configured (`STATUS_LINK_SECRET` / `WALLET_BASE_URL` unset).

### `POST /portal` (#507 — Stripe Customer Portal)

```json
{ "token": "<cus_…>.<exp>.<hmac>" }
```

Verifies the same status token, then creates a Stripe **Billing Portal** session
for the verified customer and returns `{ "url": "https://billing.stripe.com/…" }`
to redirect the browser to. The customer id comes from the token, so a user can
only open their own portal. **Errors:** `400` missing token, `401` invalid
token, `405` non-POST, `500` not configured, `502` Stripe rejected the request.

## Security hardening (SEC / #508, #458 §5)

This package's threat model: the control plane holds **AWS creds that launch
instances** and **Stripe secrets that move money**. The risks are
provisioning-without-paying, cost-runaway/abuse, AWS-cred blast radius, and
**cost-bleed from un-torn-down nodes**. The hardening:

### 1. Scoped provisioner IAM policy (`iam/provisioner-policy.json`)

A **dedicated, least-privilege** IAM policy (tighter than `botho-deploy`) granting
**only** `ec2:RunInstances` / `ec2:TerminateInstances` / `ec2:DescribeInstances` /
`ec2:CreateTags` — **no IAM, no S3, no broad EC2**. Constrained by: instance-type
`t4g.medium`, region `us-west-2`, the required tag `botho:managed-node=true`, the
specific AMI/SG/subnet/key-pair, and — critically — **`ec2:TerminateInstances`
restricted to resources tagged `botho:managed-node=true`**, so the credential can
**never** terminate the seed/seed2/faucet nodes. `CreateTags` is allowed only as
tag-on-create, so the tag can't be forged onto a non-managed instance. Full
rationale + apply steps in `iam/README.md`. Validated by `src/iam-policy.test.ts`.

### 2. Reconciliation cron (the cost-bleed safety net)

A scheduled Worker (`[triggers].crons` in `wrangler.toml`, every 15 min →
`scheduled()` → `handleScheduled` → `reconcileOnce`) that:

1. lists every `botho:managed-node=true` EC2 instance (`describeManagedNodes`, the
   same tag IAM gates terminate on — so it can only ever see/act on managed nodes),
2. reads each instance's `botho:subscription` tag and asks Stripe "is this
   subscription still **active**?" (`SubscriptionChecker`),
3. **terminates orphans** — terminate the instance, delete its DNS record, mark
   the D1 row `terminated` — for: cancelled / unpaid / absent subscriptions, a
   managed node with no subscription tag, and **stuck-provisioning** nodes (never
   reached `running` past `STUCK_PROVISIONING_MS`),
4. leaves nodes with an **active** subscription strictly alone, and
5. **skips** (never reaps) a node when the Stripe lookup errors transiently — a
   Stripe hiccup can never reap a paying customer's box.

Same injectable pattern as the provisioner (EC2 / DNS / D1 / Stripe are
interfaces), so the whole sweep is unit-tested with in-memory fakes in
`src/reconcile.test.ts` + `src/scheduled-handler.test.ts` — **no real call in a
test path**.

### 3. Caps (region / instance-type / per-subscription / global fleet)

- **Region allowlist** (`us-west-2`) + **instance-type allowlist** (`t4g.medium`),
  fail-closed, in `node-config.ts` (#502, re-verified here).
- **Global fleet cap** (`FLEET_CAP`, default 25) circuit breaker.
- **Explicit per-subscription cap**: `MAX_INSTANCES_PER_SUBSCRIPTION` (=1) is now
  enforced as a counted check (provisioner step 5b) that re-counts live instances
  carrying this `botho:subscription` tag in EC2 immediately before `RunInstances`
  and **adopts** the existing instance instead of launching a second — closing the
  narrow concurrent-launch window on top of the structural `subscription_id`-UNIQUE
  guarantee. (This wires in the previously-dead `MAX_INSTANCES_PER_SUBSCRIPTION` /
  `per_subscription_cap` symbols the prior review flagged; the latter remains in
  the error-code union as a documented alternative to the adopt policy.)
- **SigV4 known-good reference-vector test** (`src/aws-sigv4.test.ts`): the signer
  core is pinned against AWS's published Signature Version 4 worked example (the
  IAM `ListUsers` GET) — empty-payload hash, canonical-request hash, and final
  signature all byte-exact AWS's documented values.

### LIVE-mode go/no-go checklist (#458 §7)

Complete ALL before flipping Stripe from TEST to LIVE / charging real money:

- [ ] The dedicated provisioner IAM user has **only** `iam/provisioner-policy.json`
      attached (verify no extra inline/managed policies; not `botho-deploy`).
- [ ] `ec2:TerminateInstances` is confirmed tag-conditioned (the policy test passes
      AND a manual `aws iam simulate-principal-policy` denies terminate on a
      seed/faucet instance).
- [ ] The reconciliation cron is deployed and a TEST sweep correctly reaped a
      cancelled-subscription node in staging (and left active ones alone).
- [ ] All secrets (`STRIPE_*`, `AWS_*`, `CF_DNS_*`, `STATUS_LINK_SECRET`) are
      Worker secrets, rotatable, and **absent from the repo / `.dev.vars` is
      gitignored**.
- [ ] The webhook is the **only** launch trigger (no public `/provision`) and
      signature verification is enforced (#506).
- [ ] The `$50`-vs-cost economics (#441 open item a) are settled.

## Configuration (secrets & vars)

All secrets come from Worker secrets / vars — **never the repo** (#458 §2, §5).

| Key                    | Kind   | Notes                                                        |
|------------------------|--------|-------------------------------------------------------------|
| `STRIPE_SECRET_KEY`    | secret | TEST key (`sk_test_...`) while on testnet (#458 §7). Also used by the reconciliation cron to check subscription status (#508). |
| `RECONCILE_REGIONS`    | var    | Comma-separated regions the SEC cron sweeps (default = launch allowlist). |
| `STUCK_PROVISIONING_MS`| var    | Age (ms) after which a not-yet-running node is reaped as stuck (default 30 min). |
| `STRIPE_PRICE_ID`      | secret | The recurring $50/mo Price id (`price_...`). See below.     |
| `STRIPE_WEBHOOK_SECRET`| secret | Webhook signing secret (`whsec_...`). Required for `/webhook`.|
| `STATUS_LINK_SECRET`   | secret | HMAC secret for magic-link status tokens. Required for `/status` + `/portal`.|
| `CHECKOUT_SUCCESS_URL` | var    | Stripe success redirect (in `wrangler.toml`).               |
| `CHECKOUT_CANCEL_URL`  | var    | Stripe cancel redirect (in `wrangler.toml`).                |
| `WALLET_BASE_URL`      | var    | Wallet origin for the "open in wallet" deep link.           |
| `PORTAL_RETURN_URL`    | var    | Where Stripe returns after the Customer Portal closes.      |
| `ALLOWED_ORIGINS`      | var    | Comma-separated browser origins allowed to call the API.    |

### Stripe setup (one-time, TEST mode)

In the Stripe dashboard (legal entity **2amlogic**, **test mode** while on
testnet):

1. **Product:** create "Botho Managed Node (testnet)".
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

`src/status-link.test.ts`, `src/status.test.ts`, and
`src/status-handler.test.ts` cover the `/status` + `/portal` surface: token
mint/verify (round-trip, tampered customer id, wrong secret, expired), the
`node_getStatus` health summary (online/offline/never-throws), the wallet deep
link, the **authz boundary** (a user gets their own node, never another's → 404),
and the HTTP handlers (200 / 400 missing token / 401 forged token / 404 no node /
500 unconfigured) — all with an in-memory store + mocked node/Stripe fetch.

**SEC (#508):** `src/reconcile.test.ts` + `src/scheduled-handler.test.ts` cover
the reconciliation sweep — an instance whose subscription is cancelled/absent →
terminated, an active subscription → left alone, stuck-provisioning past the
threshold → terminated, a tag-less managed node → terminated, a transient Stripe
error → **skipped** (never reaps a paying node), already-terminating instances
ignored, and the guarantee it **never touches non-managed-node instances** — all
with in-memory EC2/DNS/D1/Stripe fakes. `src/stripe-subscriptions.test.ts` covers
the subscription-status client (active/cancelled/404/transient-throw) with a
mocked fetch. `src/aws-sigv4.test.ts` adds the **known-good SigV4 reference-vector
test** (pinned against AWS's published worked example). `src/iam-policy.test.ts`
asserts `iam/provisioner-policy.json` is valid and its `TerminateInstances`
statement is tag-conditioned to `botho:managed-node=true`.

## Deploy

```bash
pnpm --filter @botho/baas-worker deploy   # wrangler deploy
```
