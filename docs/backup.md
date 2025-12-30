# Backup & Recovery Guide

Protect your funds by properly backing up your wallet.

## What to Back Up

| Item | Purpose | Required? |
|------|---------|-----------|
| Mnemonic phrase | Recover wallet anywhere | **Essential** |
| config.toml | Contains mnemonic + settings | Convenient |
| Ledger database | Avoid re-sync | Optional |

**The mnemonic is the only essential backup.** Everything else can be reconstructed.

---

## Backing Up Your Mnemonic

### Step 1: Display Your Mnemonic

If you need to see your mnemonic again after initial setup:

```bash
# The mnemonic is stored in config.toml
cat ~/.botho/config.toml | grep mnemonic
```

### Step 2: Write It Down

Write all 24 words on paper:

```
1.  ________    13. ________
2.  ________    14. ________
3.  ________    15. ________
4.  ________    16. ________
5.  ________    17. ________
6.  ________    18. ________
7.  ________    19. ________
8.  ________    20. ________
9.  ________    21. ________
10. ________    22. ________
11. ________    23. ________
12. ________    24. ________
```

**Important:**
- Use pen, not pencil (pencil fades)
- Write clearly
- Double-check each word
- Number the words (order matters!)

### Step 3: Verify the Backup

Read back each word and verify against the original. One wrong word = failed recovery.

### Step 4: Store Securely

Store your paper backup in:
- A fireproof safe
- A safety deposit box
- A secure location known only to you

**Make multiple copies** stored in different locations.

---

## Backup Storage Options

### Paper Backup

**Pros:**
- Simple
- No technology required
- Free

**Cons:**
- Vulnerable to fire, water, decay
- Can be found by others

**Best practices:**
- Use acid-free paper
- Store in waterproof container
- Make multiple copies
- Store copies in different locations

### Metal Backup

**Pros:**
- Fire and water resistant
- Very durable

**Cons:**
- More effort to create
- Costs money

**Options:**
- Steel plates with letter stamps
- Commercial products (Cryptosteel, Billfodl)
- CNC engraving

### Encrypted Digital Backup

**Pros:**
- Easy to store multiple copies
- Can be stored remotely

**Cons:**
- Requires remembering encryption password
- Digital storage has risks

```bash
# Create encrypted backup
echo "your 24 word mnemonic here" | gpg --symmetric --cipher-algo AES256 -o mnemonic.gpg

# Store the .gpg file (not the original!)
# You'll need the password to decrypt

# To recover:
gpg -d mnemonic.gpg
```

**Warning:** Digital backups are risky. Prefer paper/metal for the mnemonic itself.

### Split Backup (Shamir's Secret Sharing)

Split the mnemonic into N shares where K are needed to reconstruct:

```bash
# Install ssss
sudo apt install ssss

# Split into 5 shares, 3 required to recover
echo "your-mnemonic-here" | ssss-split -t 3 -n 5

# Outputs 5 shares - distribute to trusted parties
# Any 3 shares can reconstruct the secret
```

**Use cases:**
- Business accounts with multiple signers
- Estate planning
- Geographic distribution

---

## Recovery Procedures

### Recovering on a New Machine

```bash
# Install Botho
cargo build --release

# Recover wallet
./target/release/botho init --recover

# Enter your 24-word mnemonic when prompted
# Words should be separated by spaces
```

### Recovery from config.toml Backup

```bash
# Create .botho directory
mkdir -p ~/.botho

# Copy backed up config
cp /path/to/backup/config.toml ~/.botho/

# Set permissions
chmod 600 ~/.botho/config.toml

# Start node - it will sync and find your transactions
botho run
```

### What Recovery Restores

| Restored | Not Restored |
|----------|--------------|
| All private keys | Transaction history (re-syncs) |
| Ability to spend funds | Custom settings (recreate) |
| All addresses | Pending transactions |

The blockchain contains your transaction history. After recovery, your node will re-scan and find all your funds.

---

## Automated Backups

### Backup Script

```bash
#!/bin/bash
# /usr/local/bin/botho-backup

set -e

BACKUP_DIR="${BACKUP_DIR:-/backup/botho}"
DATE=$(date +%Y%m%d_%H%M%S)
RETENTION_DAYS=30

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Backup config (contains mnemonic)
CONFIG_BACKUP="$BACKUP_DIR/config_$DATE.toml"
cp ~/.botho/config.toml "$CONFIG_BACKUP"

# Encrypt the backup
gpg --batch --yes --symmetric \
    --cipher-algo AES256 \
    --passphrase-file /root/.backup-passphrase \
    "$CONFIG_BACKUP"

# Remove unencrypted version
rm "$CONFIG_BACKUP"

# Clean old backups
find "$BACKUP_DIR" -name "config_*.gpg" -mtime +$RETENTION_DAYS -delete

echo "Backup complete: ${CONFIG_BACKUP}.gpg"
```

### Schedule with Cron

```bash
# Edit crontab
crontab -e

# Add daily backup at 2 AM
0 2 * * * /usr/local/bin/botho-backup >> /var/log/botho-backup.log 2>&1
```

### Remote Backup

```bash
#!/bin/bash
# Backup to remote server

LOCAL_BACKUP="/backup/botho/config_latest.gpg"
REMOTE="backup@remote-server:/backups/botho/"

# Create latest backup
/usr/local/bin/botho-backup
cp "$(ls -t /backup/botho/config_*.gpg | head -1)" "$LOCAL_BACKUP"

# Sync to remote
rsync -avz "$LOCAL_BACKUP" "$REMOTE"
```

---

## Verification

### Verify Backup Integrity

Periodically verify your backup works:

```bash
# On a test machine or VM (NOT your main machine)
botho init --recover
# Enter your backed-up mnemonic

# Verify the address matches
botho address
# Should match your known address
```

### Verify Encrypted Backups

```bash
# Test decryption
gpg -d /backup/botho/config_latest.gpg > /dev/null
# Should succeed without errors
```

---

## Disaster Recovery

### Scenario: Computer Destroyed

1. Obtain a new computer
2. Install Botho from source or trusted binary
3. Recover wallet using mnemonic
4. Wait for blockchain to sync
5. Verify balance

### Scenario: Mnemonic Lost/Destroyed

If you lose all copies of your mnemonic:
- **If you still have config.toml**: Extract mnemonic and create new backups immediately
- **If you've lost everything**: Funds are unrecoverable

This is why multiple backup copies in different locations are essential.

### Scenario: Mnemonic Compromised

If someone else may have your mnemonic:

1. **Immediately** create a new wallet
2. Transfer all funds to the new wallet
3. Securely destroy the old mnemonic
4. Create new backups for the new wallet

### Scenario: Partial Mnemonic Recovery

If you only have some words:
- 23 words: Possible to brute-force the last word (2048 attempts)
- 22 words: Very difficult (4 million combinations)
- Fewer: Practically impossible

Tools exist for partial recovery, but require significant computation.

---

## Best Practices Summary

### DO

- [ ] Write mnemonic on paper immediately after wallet creation
- [ ] Store multiple copies in different physical locations
- [ ] Use fireproof and waterproof storage
- [ ] Verify backup by reading it back
- [ ] Periodically verify you can still access backups
- [ ] Consider metal backup for durability

### DON'T

- [ ] Store mnemonic digitally (unencrypted)
- [ ] Take photos of your mnemonic
- [ ] Store only one copy
- [ ] Share your mnemonic with anyone
- [ ] Store mnemonic with your computer
- [ ] Forget where you put backups

---

## Backup Checklist

### Initial Setup

- [ ] Wallet created
- [ ] Mnemonic written on paper
- [ ] Mnemonic verified (read back)
- [ ] Backup copy created
- [ ] Primary backup stored securely
- [ ] Secondary backup in different location
- [ ] config.toml permissions set (600)

### Periodic Verification (Monthly)

- [ ] Can locate all backup copies
- [ ] Backups are intact and readable
- [ ] Recovery process tested (on test machine)
- [ ] Encrypted backups still decryptable

### After Security Event

- [ ] Assess if mnemonic may be compromised
- [ ] If compromised: create new wallet and transfer funds
- [ ] Create new backups
- [ ] Securely destroy old backups

---

## Related Documentation

- [Security Guide](security.md) — Complete security practices
- [Getting Started](getting-started.md) — Initial wallet setup
- [Troubleshooting](troubleshooting.md) — Recovery issues
