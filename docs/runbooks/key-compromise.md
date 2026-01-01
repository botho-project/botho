# Runbook: Key Compromise Response

Emergency procedure when wallet keys may be compromised.

**Target RTO:** 15-30 minutes (to secure funds)
**Severity:** Critical
**Owner:** Security

---

## Detection

### Indicators of Compromise

- Unauthorized transactions from your address
- Mnemonic exposed in logs, screenshots, or public repos
- Backup storage breached
- Device with wallet access lost or stolen
- Suspicious login activity on systems with wallet access
- Social engineering attempt reported

### Verification

Before assuming compromise, verify:

```bash
# Check recent transactions
botho history

# Check current balance
botho balance

# Look for unexpected outgoing transactions
```

**If unauthorized transactions exist → PROCEED IMMEDIATELY**

---

## Immediate Response

### Step 1: Prepare New Wallet (FIRST)

**Do this before anything else to minimize exposure time.**

On a **CLEAN, SECURE** device:

```bash
# Install Botho (if not present)
cargo build --release

# Create new wallet with fresh mnemonic
./target/release/botho init

# IMMEDIATELY write down the new mnemonic
# Store securely BEFORE proceeding
```

**CRITICAL:** Verify you have securely backed up the NEW mnemonic before continuing.

### Step 2: Transfer Funds

Transfer all funds from compromised wallet to new wallet:

```bash
# Get new wallet address
botho address  # On new wallet

# From compromised wallet, send ALL funds
botho send --to <NEW_ADDRESS> --amount <FULL_BALANCE>

# Note: Leave enough for transaction fee
# Check fee with: botho estimate-fee
```

**If attacker is actively draining:**
- Work faster
- Transfer in one transaction if possible
- Monitor mempool for competing transactions

### Step 3: Verify Transfer

```bash
# On new wallet, check for incoming transaction
botho sync
botho balance

# Wait for confirmation (1-2 blocks, ~10-15 seconds)
```

### Step 4: Secure Compromised Wallet

Once funds are safe:

```bash
# On compromised system
# Remove wallet config
rm ~/.botho/config.toml

# Clear any cached data
rm -rf ~/.botho/mainnet

# If system may be compromised, consider:
# - Full system wipe
# - Forensic analysis first if needed
```

---

## Investigation

After funds are secured:

### Determine Scope

```bash
# What systems had access to mnemonic?
# - Development machines
# - Servers
# - Backup storage
# - CI/CD systems

# Who had access?
# - Team members
# - Service accounts
# - Third-party services
```

### Check for Exposure

```bash
# Search git history for mnemonic patterns
git log -p | grep -i "mnemonic\|seed phrase"

# Check for accidental commits
git log --all --full-history -- "**/config.toml"

# Review backup locations
ls -la /backup/botho/
```

### Review Logs

```bash
# Check for unauthorized access
sudo journalctl -u botho --since "1 week ago" | grep -i "auth\|login\|access"

# Check system logs
sudo grep -r "botho" /var/log/auth.log
```

---

## Post-Incident Actions

### 1. Revoke Old Keys

The compromised mnemonic can never be used again safely:

- [ ] Remove from all systems
- [ ] Delete from all backups
- [ ] Revoke any API keys generated from it
- [ ] Update any services that reference the old address

### 2. Secure New Wallet

- [ ] New mnemonic backed up securely (metal backup recommended)
- [ ] Multiple copies in different locations
- [ ] Access restricted to authorized personnel only
- [ ] File permissions secured (chmod 600)

### 3. Update Configuration

```bash
# On production systems
# Update config with new wallet

# Restart services
sudo systemctl restart botho
```

### 4. Notify Stakeholders

- [ ] Notify team of address change
- [ ] Update any payment integrations
- [ ] Update exchange whitelists if applicable
- [ ] Document the incident

---

## Prevention

### Secure Storage Practices

**DO:**
- Store mnemonic offline (paper, metal)
- Use hardware security modules for production
- Encrypt digital backups with strong passphrase
- Limit access to need-to-know basis

**DON'T:**
- Store mnemonic in plain text
- Commit mnemonic to version control
- Share mnemonic via email, chat, or cloud storage
- Take screenshots of mnemonic

### Access Controls

```bash
# Restrict config file permissions
chmod 600 ~/.botho/config.toml

# Restrict data directory
chmod 700 ~/.botho
```

### Monitoring

Set up alerts for:
- Large outgoing transactions
- Transactions to unknown addresses
- Login attempts to wallet systems

---

## Recovery Without Funds

If attacker drained funds before you could act:

### Document Everything

1. Transaction hashes of theft
2. Destination addresses
3. Timeline of events
4. Any identifying information

### Report

1. File incident report with law enforcement (if applicable)
2. Report addresses to blockchain analytics services
3. Monitor destination addresses for exchange deposits

### Learn and Improve

1. Conduct post-mortem
2. Identify how compromise occurred
3. Implement additional safeguards
4. Update security procedures

---

## Escalation Matrix

| Time | Action |
|------|--------|
| 0-5 min | Create new wallet, begin transfer |
| 5-15 min | Complete fund transfer, verify |
| 15-30 min | Secure old wallet, begin investigation |
| 30+ min | Post-incident actions, stakeholder notification |

### Contacts

| Role | When to Contact |
|------|-----------------|
| Security Lead | Immediately |
| Infrastructure Lead | If system access needed |
| Legal | If significant loss |
| Law Enforcement | If criminal activity suspected |

---

## Related Documentation

- [Backup & Recovery Guide](../backup.md)
- [Security Guide](../security.md)
