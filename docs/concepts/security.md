# Security Guide

Best practices for securing your Botho wallet and node.

## Overview

Security in Botho operates at multiple layers:

| Layer | Protection | Your Responsibility |
|-------|------------|---------------------|
| Cryptographic | Transaction privacy, quantum resistance | Keep keys safe |
| Application | Input validation, secure defaults | Keep software updated |
| Operational | Key management, access control | Follow best practices |
| Network | Peer authentication, rate limiting | Configure firewall |

---

## Threat Model

### What Botho Protects Against

| Threat | Protection |
|--------|------------|
| Transaction surveillance | Stealth addresses, ring signatures |
| Amount analysis | Confidential transactions (Pedersen + Bulletproofs) |
| Quantum computers (recipients) | ML-KEM-768 stealth addresses |
| Quantum computers (minting) | ML-DSA-65 signatures |
| Double-spending | SCP consensus, key images |
| Replay attacks | Tombstone blocks |
| Network-level attacks | Peer reputation, rate limiting |

### What You Must Protect Against

| Threat | Your Action |
|--------|-------------|
| Key theft | Secure mnemonic storage |
| Malware | Secure computing environment |
| Physical access | Encrypt storage, secure location |
| Social engineering | Verify addresses, be skeptical |
| Supply chain | Verify downloads, build from source |

---

## Key Management

### The Mnemonic Phrase

Your 24-word mnemonic is the master key to your funds. Anyone with these words can steal everything.

**DO:**
- Write it on paper (multiple copies)
- Store copies in separate secure locations
- Consider a fireproof safe or safety deposit box
- Memorize it if possible (as backup)

**DON'T:**
- Store it digitally (computer, phone, cloud)
- Take photos of it
- Email or message it to anyone
- Enter it on any website
- Share it with "support" staff

### Mnemonic Storage Options

| Method | Security | Durability | Cost |
|--------|----------|------------|------|
| Paper (multiple copies) | Medium | Low (fire/water) | Free |
| Metal plate engraving | Medium | High | $20-50 |
| Safety deposit box | High | High | $50/year |
| Hardware wallet backup | High | Medium | $50-150 |
| Split with Shamir's | Very High | Medium | Complex |

### Example: Metal Backup

```
┌─────────────────────────────────────┐
│  1. abandon   13. lens             │
│  2. ability   14. liberty          │
│  3. able      15. light            │
│  ...         ...                   │
│  12. about    24. zoo              │
└─────────────────────────────────────┘
Engrave on stainless steel plate
Store in fireproof safe
```

### Passphrase (Optional Extra Security)

BIP39 supports an optional passphrase that acts as a "25th word":

```bash
# Recovery with passphrase
botho init --recover
# Enter mnemonic, then passphrase when prompted
```

**Benefits:**
- Same mnemonic + different passphrases = different wallets
- Provides plausible deniability (decoy wallet)
- Adds protection if mnemonic is compromised

**Risks:**
- Forgotten passphrase = lost funds (unrecoverable)
- Must backup passphrase separately from mnemonic

---

## Secure Configuration

### Config File Permissions

```bash
# The config file contains your mnemonic
chmod 600 ~/.botho/config.toml

# Verify
ls -la ~/.botho/config.toml
# Should show: -rw------- (only owner can read/write)
```

### Data Directory Permissions

```bash
chmod 700 ~/.botho
```

### Minimal config.toml

```toml
[wallet]
# Mnemonic here - ensure file permissions are 600

[network]
gossip_port = 7100
rpc_port = 7101

# Only allow local RPC access
cors_origins = ["http://localhost", "http://127.0.0.1"]

bootstrap_peers = [
    "/ip4/98.95.2.200/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ",
]

[network.quorum]
mode = "recommended"
min_peers = 2

[minting]
enabled = false
```

---

## Operational Security

### Verify Software Integrity

```bash
# Clone from official repository
git clone https://github.com/botho-project/botho.git

# Verify you're on a tagged release
git tag -v v0.1.0

# Build from source
cargo build --release

# Check binary hash (compare with published hash)
sha256sum target/release/botho
```

### Secure Computing Environment

**Dedicated Machine (Ideal):**
- Use a dedicated computer for cryptocurrency
- Minimal software installed
- No web browsing or email
- Regular security updates

**Shared Machine (Minimum):**
- Keep OS and software updated
- Use antivirus/antimalware
- Don't run untrusted software
- Use a separate user account

### Network Security

```bash
# Only expose P2P port publicly
# Keep RPC port local-only or behind firewall

# UFW example
sudo ufw allow 7100/tcp  # P2P - public
# RPC stays local (no ufw rule = blocked)
```

### Air-Gapped Signing (Maximum Security)

For large holdings, consider offline signing:

1. **Online machine**: Syncs blockchain, creates unsigned transactions
2. **Offline machine**: Signs transactions (never connects to network)
3. **Transfer**: Signed transaction moved via USB to online machine

---

## RPC Security

### Local-Only Access (Default)

The RPC server binds to all interfaces but should be firewalled:

```bash
# Only allow localhost
iptables -A INPUT -p tcp --dport 7101 -s 127.0.0.1 -j ACCEPT
iptables -A INPUT -p tcp --dport 7101 -j DROP
```

### CORS Configuration

Restrict which websites can access your RPC:

```toml
[network]
# Bad - allows any website
cors_origins = ["*"]

# Good - only your domains
cors_origins = ["https://yourdomain.com"]

# Best - local only
cors_origins = ["http://localhost", "http://127.0.0.1"]
```

### Rate Limiting

If exposing RPC publicly, use nginx rate limiting:

```nginx
limit_req_zone $binary_remote_addr zone=rpc:10m rate=10r/s;

location / {
    limit_req zone=rpc burst=20 nodelay;
    proxy_pass http://127.0.0.1:7101;
}
```

---

## Transaction Security

### Verify Addresses

Before sending:
- Double-check the recipient address
- Verify through a second channel if possible
- Start with a small test transaction

### Avoid Address Reuse

Botho uses stealth addresses, so each transaction creates a unique address. However:
- Generate new receiving addresses for each payment request
- Use subaddresses for organization

### Confirm Before Sending

```bash
# The send command shows a confirmation
botho send <address> <amount>
# Review carefully before confirming
```

---

## Privacy Best Practices

### Network Privacy

Botho protects transaction privacy but not network privacy:

| Visible | Hidden |
|---------|--------|
| Your IP address | Sender identity |
| That you run a node | Recipient identity |
| Transaction timing | Transaction amounts |

**For network privacy:**
- Use Tor: `torsocks botho run`
- Use a VPN
- Use a cloud server

### Timing Correlation

Avoid patterns that could deanonymize you:
- Don't always transact at the same time
- Don't immediately spend received funds
- Allow time between related transactions

### Metadata Leakage

Be careful about:
- Sharing addresses publicly with your identity
- Posting transaction hashes on social media
- Correlating exchange deposits/withdrawals

---

## Incident Response

### If Your Mnemonic is Compromised

**Immediately:**
1. Create a new wallet: `botho init`
2. Transfer all funds to the new wallet
3. Securely destroy the old mnemonic
4. Investigate how compromise occurred

### If Your Node is Compromised

1. Stop the node immediately
2. Don't use the compromised machine
3. Check if config.toml was accessed (mnemonic)
4. If mnemonic may be exposed, treat as compromised
5. Set up fresh node on clean machine

### If You Sent to Wrong Address

Unfortunately, transactions are irreversible. This is why verification is critical before sending.

---

## Security Checklist

### Initial Setup
- [ ] Generated wallet on secure machine
- [ ] Wrote mnemonic on paper (2+ copies)
- [ ] Stored copies in separate secure locations
- [ ] Set config.toml permissions to 600
- [ ] Set data directory permissions to 700
- [ ] Verified software integrity

### Ongoing
- [ ] Keep software updated
- [ ] Monitor for unusual activity
- [ ] Regularly verify backup accessibility
- [ ] Review connected applications periodically
- [ ] Check file permissions haven't changed

### Before Transactions
- [ ] Verify recipient address carefully
- [ ] Confirm amount is correct
- [ ] Use test transaction for new recipients
- [ ] Check you're on the correct network

---

## Security Contacts

### Reporting Vulnerabilities

If you discover a security vulnerability:

1. **DO NOT** disclose publicly
2. Email security details to the maintainers
3. Allow time for a fix before disclosure
4. See [SECURITY.md](../../SECURITY.md) for full policy

### Getting Help

- GitHub Issues (non-sensitive): [github.com/botho-project/botho/issues](https://github.com/botho-project/botho/issues)
- Security issues: Follow responsible disclosure

---

## Advanced Topics

### Hardware Security Modules (HSM)

For institutional deployments, consider:
- AWS CloudHSM
- Azure Dedicated HSM
- YubiHSM

These store keys in tamper-resistant hardware.

### Multi-Signature (Future)

Multi-sig wallets require multiple keys to authorize transactions. This is planned for future Botho versions.

### Shamir's Secret Sharing

Split your mnemonic into N shares where K are needed to reconstruct:

```
Mnemonic → [Share 1] [Share 2] [Share 3] [Share 4] [Share 5]
                         3-of-5 required to recover
```

Tools like `ssss` can help, but understand the risks:
- More complex to manage
- Must trust share holders
- Must maintain enough shares

---

## Further Reading

- [Backup & Recovery](../operations/backup.md) — Detailed backup procedures
- [Deployment](../operations/deployment.md) — Production deployment security
- [Privacy Features](privacy.md) — How Botho protects privacy
- [SECURITY.md](../../SECURITY.md) — Vulnerability disclosure policy
