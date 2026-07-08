-- Botho-as-a-Service control-plane D1 schema (#458 §3 step 4, #502).
--
-- Apply locally:
--   wrangler d1 execute botho-baas --local --file=schema.sql
-- Apply to the remote D1 database:
--   wrangler d1 execute botho-baas --remote --file=schema.sql
--
-- The `nodes` table is the user<->node mapping. `subscription_id` is UNIQUE and is
-- the idempotency anchor: the provisioner checks it BEFORE launching so a
-- replayed Stripe trigger never creates a second instance (#458 §3, §5).

CREATE TABLE IF NOT EXISTS nodes (
  -- Stripe customer id == our user identity (#458 §4).
  user             TEXT    NOT NULL,
  -- Stripe customer id (denormalized for lookups / portal).
  stripe_customer  TEXT    NOT NULL,
  -- Stripe subscription id — the idempotency key.
  subscription_id  TEXT    NOT NULL UNIQUE,
  -- Short opaque node id used for the hostname (node-<id>.<domain>).
  node_id           TEXT    NOT NULL,
  -- EC2 instance id once launched (NULL while still pre-launch).
  instance_id      TEXT,
  -- AWS region (constrained to the allowlist by the provisioner).
  region           TEXT    NOT NULL,
  -- HTTPS /rpc URL the user points the PWA at.
  rpc_url          TEXT    NOT NULL,
  -- Lifecycle: provisioning -> running -> suspended -> terminated.
  state            TEXT    NOT NULL DEFAULT 'provisioning'
                     CHECK (state IN ('provisioning','running','suspended','terminated')),
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_nodes_state ON nodes (state);
CREATE INDEX IF NOT EXISTS idx_nodes_customer ON nodes (stripe_customer);
CREATE INDEX IF NOT EXISTS idx_nodes_instance ON nodes (instance_id);
