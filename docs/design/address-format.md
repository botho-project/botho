# Botho Address Format Specification

This document specifies the address format used in the Botho protocol.

## Overview

Botho uses a URI-based address format that encodes network, version, and cryptographic key material:

```
tbotho://1/NTMFPKArvQw4Z8K7YhPdF9xmGn2jL5bRc...
│        │ └─ base58-encoded public keys (view || spend)
│        └─── version number (address format)
└──────────── network identifier
```

## URI Scheme

| Network  | URI Scheme | Example |
|----------|------------|---------|
| Testnet  | `tbotho://` | `tbotho://1/NTMFPKArv...` |
| Mainnet  | `botho://`  | `botho://1/NTMFPKArv...` |

### Rationale for Separate URI Schemes

Using distinct URI schemes for testnet vs mainnet provides:

1. **Maximum safety** - Impossible to confuse networks at the protocol level
2. **QR code clarity** - Wallets immediately identify the target network when scanning
3. **Copy-paste protection** - Mainnet wallets reject `tbotho://` addresses entirely
4. **Clear user experience** - Users can visually identify network at a glance

This is more robust than Bitcoin's approach (address prefix only) because the URI scheme itself enforces network separation.

## Version Number

The path component starts with a version number:

| Version | Format | Status |
|---------|--------|--------|
| `1`     | Ristretto255 dual-key | Current |

The version number allows future address format upgrades (e.g., post-quantum cryptography) without breaking existing addresses.

## Address Payload

### Structure

```
base58(view_public_key || spend_public_key)
```

- **view_public_key**: 32 bytes, Ristretto255 compressed point
- **spend_public_key**: 32 bytes, Ristretto255 compressed point
- **Total**: 64 bytes before base58 encoding

### Encoding

Base58 encoding (Bitcoin alphabet) is used for:
- Human readability (no ambiguous characters: 0, O, I, l)
- URL safety (no special characters)
- Compact representation

## Key Derivation

Addresses are derived from BIP39 mnemonics following this path:

```
Mnemonic
    │
    ▼ BIP39 (empty passphrase)
64-byte Seed
    │
    ▼ SLIP-10 Ed25519
m/44'/866'/account'
    │
    ├─▶ HKDF-SHA512 (salt: "botho-ristretto255-view")
    │       │
    │       ▼ mod L (curve order)
    │   View Private Key (32 bytes)
    │       │
    │       ▼ scalar × G
    │   View Public Key (32 bytes)
    │
    └─▶ HKDF-SHA512 (salt: "botho-ristretto255-spend")
            │
            ▼ mod L (curve order)
        Spend Private Key (32 bytes)
            │
            ▼ scalar × G
        Spend Public Key (32 bytes)
```

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| BIP44 Purpose | `44'` | Standard BIP44 |
| Coin Type | `866'` | Botho's registered coin type |
| Curve Order (L) | `2^252 + 27742317777372353535851937790883648493` | Ed25519/Ristretto255 |

### Domain Separators

```
VIEW_DOMAIN  = "botho-ristretto255-view"
SPEND_DOMAIN = "botho-ristretto255-spend"
```

## Validation Rules

An address is valid if:

1. URI scheme is `tbotho://` or `botho://`
2. Version is a supported version number (currently only `1`)
3. Payload decodes to exactly 64 bytes via base58
4. Both 32-byte keys are valid Ristretto255 points (on-curve)

## Example

```
Mnemonic: abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about

Testnet Address: tbotho://1/NTMFPKArvQw4Z8K7YhPdF9xmGn2jL5bRcVtHwXkEqP3sY6mDfA8nU9oJzK1iT4gW

Components:
  - Network: testnet
  - Version: 1
  - View Public:  [32 bytes]
  - Spend Public: [32 bytes]
```

## Security Considerations

1. **Network isolation**: The URI scheme difference prevents cross-network transaction errors
2. **Dual-key privacy**: Separate view and spend keys enable view-only wallets
3. **Deterministic derivation**: Same mnemonic always produces same address
4. **Domain separation**: HKDF domain separators prevent key reuse attacks

## Comparison with Other Cryptocurrencies

| Feature | Botho | Bitcoin | Monero | Ethereum |
|---------|-------|---------|--------|----------|
| Network separation | URI scheme | Address prefix | Address prefix | None |
| Version support | URI path | Address prefix | Implicit | None |
| Key type | Dual (view+spend) | Single | Dual | Single |
| Encoding | base58 | bech32/base58 | base58 | hex |

## References

- [BIP39: Mnemonic code for generating deterministic keys](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki)
- [BIP44: Multi-Account Hierarchy for Deterministic Wallets](https://github.com/bitcoin/bips/blob/master/bip-0044.mediawiki)
- [SLIP-10: Universal private key derivation from master private key](https://github.com/satoshilabs/slips/blob/master/slip-0010.md)
- [Ristretto255: A group derived from Curve25519](https://ristretto.group/)
