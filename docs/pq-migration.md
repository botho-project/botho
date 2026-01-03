# Post-Quantum Migration Guide

This guide explains how to upgrade your Botho funds to support LION post-quantum ring signatures for quantum-safe sender privacy.

## Do I Need to Migrate?

**Yes, if you want to use LION ring signatures for sender privacy.**

LION ring signatures require LION public keys (1,312 bytes each) for all ring members. Standard-Private UTXOs only have classical Ristretto keys (32 bytes). To spend with LION, you first need UTXOs that have LION key material.

### What Migration Does

1. Spends your existing Standard-Private UTXOs using CLSAG
2. Creates new outputs with both classical AND LION key material
3. After migration, you can spend with either CLSAG or LION

### After Migration

Once migrated, you have flexibility:

```bash
# Use CLSAG for everyday transactions (cheaper, ~0.7 KB/input)
botho-wallet send <address> <amount>

# Use LION for quantum-safe sender privacy (~36 KB/input)
botho-wallet send <address> <amount> --quantum-private
```

### Do I Need to Migrate Everything?

**No.** You can:
- Keep some funds as Standard-Private (smaller outputs, lower fees)
- Migrate funds you want quantum-safe sender privacy for
- Mix both types in your wallet

Choose based on your threat model. For "harvest now, decrypt later" protection, migrated funds can use LION when spent.

---

## Why Migrate?

### The Quantum Threat

Current classical cryptographic algorithms (like the elliptic curve cryptography used in Bitcoin, Monero, and classical Botho addresses) are vulnerable to quantum computers running Shor's algorithm. While large-scale quantum computers don't exist today, the threat is real:

- **Harvest Now, Decrypt Later (HNDL)**: Adversaries can record encrypted transactions today and decrypt them once quantum computers become available.
- **NIST Timeline**: NIST estimates cryptographically relevant quantum computers (CRQC) could emerge within 10-15 years.
- **Long-term Protection**: If you hold funds for years, migration now ensures they remain protected regardless of when quantum computers arrive.

### Botho's Solution

Botho implements a hybrid post-quantum security model using NIST-standardized algorithms:

- **ML-KEM-768 (Kyber)**: For key encapsulation (stealth address key exchange)
- **ML-DSA-65 (Dilithium)**: For digital signatures (transaction authentication)

Quantum-private transactions require BOTH classical and post-quantum signatures to verify, providing:

1. Immediate protection against HNDL attacks
2. Fallback security if either cryptosystem is broken
3. Backward compatibility with existing infrastructure

## Before You Begin

### Prerequisites

1. **Same Seed = Same Keys**: Your existing 24-word mnemonic generates BOTH classical and quantum-safe keys. **No new backup is required.**
2. **Wallet Balance**: Ensure you have sufficient balance to cover migration transaction fees.
3. **Node Synced**: Your wallet should be synced to the current network height.
4. **Build with PQ Feature**: Ensure your wallet was built with the `pq` feature enabled:
   ```bash
   cargo build --release --features pq
   ```

### Understanding Address Formats

| Type | Prefix | Size | Example |
|------|--------|------|---------|
| Classical | `cad:` | ~95 chars | `cad:a1b2c3...d4e5f6:g7h8i9...j0k1l2` |
| Quantum-Safe | `botho-pq://1/` | ~4.3KB | `botho-pq://1/<base58-encoded>` |

The quantum-safe address is larger because it includes:
- Classical view key (32 bytes)
- Classical spend key (32 bytes)
- ML-KEM-768 public key (1,184 bytes)
- ML-DSA-65 public key (1,952 bytes)
- **Total: ~3,200 bytes**

## Step-by-Step Migration

### Step 1: View Your Quantum-Safe Address

First, display your quantum-safe address to verify PQ functionality is working:

```bash
botho-wallet address --pq
```

This will show both your classical and quantum-safe addresses. The quantum-safe address is derived from the same mnemonic - no separate backup needed.

### Step 2: Check Current Balance

Verify your wallet balance before migration:

```bash
botho-wallet balance --detailed
```

Note the total balance and UTXO count. The migration will consolidate all classical UTXOs into quantum-safe outputs.

### Step 3: Preview Migration (Dry Run)

Preview what the migration will do without making changes:

```bash
botho-wallet migrate-to-pq --dry-run
```

This shows:
- Number of classical UTXOs to migrate
- Total amount being migrated
- Estimated fees
- Expected number of migration transactions

### Step 4: Execute Migration

When ready, execute the migration:

```bash
botho-wallet migrate-to-pq
```

Or to skip the confirmation prompt:

```bash
botho-wallet migrate-to-pq --yes
```

The command will:
1. Find all classical UTXOs in your wallet
2. Create quantum-private transaction(s) sending them to your PQ address
3. Wait for confirmation
4. Update your wallet state

### Step 5: Verify Migration

Check the migration status:

```bash
botho-wallet migrate-to-pq --status
```

This shows:
- Classical balance (should be 0 after complete migration)
- Quantum-safe balance
- Pending migration transactions (if any)

You can also verify with:

```bash
botho-wallet balance --detailed
```

### Step 6: Update Addresses with Counterparties

After migration, share your new quantum-safe address with:
- Exchanges (for deposits)
- Payment processors
- Anyone who sends you BTH regularly

Your classical address still works, but new deposits to it will need to be migrated again.

## Frequently Asked Questions

### Do I need a new seed phrase?

**No.** Your existing 24-word mnemonic generates both classical and quantum-safe keys deterministically. The same seed phrase recovers both address types. No additional backup is needed.

### What if quantum computers never come?

Quantum-private transactions work exactly like classical transactions - your funds remain spendable regardless. The classical signature layer continues to provide security even if post-quantum cryptography turns out to be unnecessary.

### Can I switch back to classical addresses?

**Yes.** You can sweep quantum-safe UTXOs back to classical addresses at any time:

```bash
botho-wallet send <classical-address> <amount>
```

However, this removes quantum protection from those funds.

### Why are quantum-safe addresses so large?

Post-quantum public keys are significantly larger than classical keys:

| Key Type | Size |
|----------|------|
| Classical (Ristretto) | 32 bytes |
| ML-KEM-768 (view) | 1,184 bytes |
| ML-DSA-65 (spend) | 1,952 bytes |

The ~4.3KB address size is the base58 encoding of all these keys combined.

### How do I share my PQ address?

For large addresses, use one of these methods:

1. **QR Code**: Generate a QR code for easy scanning
   ```bash
   botho-wallet address --pq --qr
   ```

2. **Copy/Paste**: The full address string works in any text field

3. **File**: Export the address to a file
   ```bash
   botho-wallet address --pq > my-pq-address.txt
   ```

4. **Shortened Reference**: Some applications support address book lookups

### Are migration fees higher?

**Yes, but only slightly.** Quantum-private transactions are approximately 19x larger than classical transactions due to larger signatures and key material. However:

- The fee structure scales with transaction size
- Migration is a one-time cost
- Future quantum-private transactions have the same fee structure

### What about dust UTXOs?

Small UTXOs (below the dust threshold) may not be worth migrating individually. The migration command:

1. **Consolidates**: Batches small UTXOs together to reduce fees
2. **Absorbs Dust**: Very small amounts may be absorbed into the fee rather than creating unspendable outputs
3. **Skips if Empty**: Reports "nothing to migrate" if the wallet has no balance

### Is the migration atomic?

For wallets with many UTXOs, migration may require multiple transactions. The command:

1. Processes UTXOs in batches
2. Tracks migration progress
3. Can be resumed if interrupted
4. Shows partial progress with `--status`

### What happens during a network fork?

Quantum-private transactions are valid on both sides of a consensus fork, just like classical transactions. Your migrated funds remain safe.

## Troubleshooting

### "Quantum-safe addresses are not enabled in this build"

Rebuild the wallet with the PQ feature:

```bash
cargo build --release --features pq -p botho-wallet
```

### "Insufficient funds for migration"

You need enough balance to cover:
- Migration transaction fees (~19x larger than classical fees)
- Minimum output amount

Check your balance with `botho-wallet balance` and ensure it exceeds the fee estimate shown in `--dry-run`.

### "No classical UTXOs to migrate"

This means your wallet either:
- Has no balance
- Has already been fully migrated
- Only contains quantum-safe UTXOs

Use `botho-wallet balance --detailed` to see the breakdown.

### "Transaction failed"

Migration transactions can fail for the same reasons as regular transactions:
- Network connectivity issues
- Node synchronization problems
- Fee estimation errors

The migration is safe - your funds remain in classical UTXOs until successfully migrated. Retry the command after resolving the issue.

### "Migration taking too long"

Large wallets with many UTXOs may take several minutes. You can:
1. Use `--status` to check progress
2. Let it run to completion
3. The process is resumable if interrupted

## Technical Details

### Key Derivation

Both classical and quantum-safe keys are derived from your BIP39 mnemonic:

```
Mnemonic (24 words)
    │
    ├──► BIP39 Seed (512 bits with PBKDF2)
    │        │
    │        ├──► SLIP-0010 ──► Classical AccountKey (Ristretto)
    │        │
    │        └──► HKDF ──► PQ Keys (ML-KEM-768 + ML-DSA-65)
    │
    └── Same seed = same addresses (deterministic)
```

### Transaction Structure

A quantum-private migration transaction contains:

```
Inputs:
  - Classical UTXO references (ring signatures)

Outputs:
  - Quantum-safe output (ML-KEM encapsulated)
  - Change output (also quantum-safe)

Signatures:
  - Classical: CLSAG ring signature (Schnorr-based)
  - Post-Quantum: ML-DSA-65 signature

Size: ~19x larger than classical transaction
```

### Security Guarantees

After migration, your funds are protected by:

1. **Classical Layer**: Elliptic curve discrete log assumption (ECDLP)
2. **Post-Quantum Layer**: Lattice-based hardness (Module-LWE, Module-SIS)

An attacker would need to break BOTH layers to steal funds - classical attacks fail against the PQ layer, and quantum attacks fail against ongoing classical verification.

## Next Steps

After migration:

1. **Verify** your quantum-safe balance with `botho-wallet balance`
2. **Update** your addresses with exchanges and counterparties
3. **Monitor** the quantum computing landscape (Botho will provide updates)
4. **Use quantum-private transactions** for future sends when sending to PQ addresses:
   ```bash
   botho-wallet send <pq-address> <amount> --quantum-private
   ```

For additional help, see the [Botho documentation](./README.md) or open an issue on GitHub.
