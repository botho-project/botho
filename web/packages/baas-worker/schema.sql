-- Botho-as-a-Service control-plane D1 schema (#458 §3 step 4, #502).
--
-- Apply locally:
--   wrangler d1 execute botho-baas --local --file=schema.sql
-- Apply to the remote D1 database:
--   wrangler d1 execute botho-baas --remote --file=schema.sql
--
-- The `rigs` table is the user<->rig mapping. `subscription_id` is UNIQUE and is
-- the idempotency anchor: the provisioner checks it BEFORE launching so a
-- replayed Stripe trigger never creates a second instance (#458 §3, §5).

CREATE TABLE IF NOT EXISTS rigs (
  -- Stripe customer id == our user identity (#458 §4).
  user             TEXT    NOT NULL,
  -- Stripe customer id (denormalized for lookups / portal).
  stripe_customer  TEXT    NOT NULL,
  -- Stripe subscription id — the idempotency key.
  subscription_id  TEXT    NOT NULL UNIQUE,
  -- Short opaque rig id used for the hostname (rig-<id>.<domain>).
  rig_id           TEXT    NOT NULL,
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

CREATE INDEX IF NOT EXISTS idx_rigs_state ON rigs (state);
CREATE INDEX IF NOT EXISTS idx_rigs_customer ON rigs (stripe_customer);
CREATE INDEX IF NOT EXISTS idx_rigs_instance ON rigs (instance_id);
