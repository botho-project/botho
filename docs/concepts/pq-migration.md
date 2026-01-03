# Post-Quantum Migration Guide (Deprecated)

> **Note**: This document describes migration to LION ring signatures, which has been **deprecated**. See [ADR-0001](../decisions/0001-deprecate-lion-ring-signatures.md) for details.

## Current Post-Quantum Architecture

Botho now uses a simpler hybrid approach that doesn't require migration:

| Component | Algorithm | Security Level |
|-----------|-----------|----------------|
| Stealth addresses | ML-KEM-768 | Post-quantum (permanent recipient privacy) |
| Minting signatures | ML-DSA-65 | Post-quantum |
| Ring signatures | CLSAG (ring=20) | Classical (ephemeral sender privacy) |
| Amount hiding | Pedersen commitments | Information-theoretic |

### Why No Migration Is Needed

1. **Recipient privacy is already post-quantum**: All stealth addresses use ML-KEM-768
2. **Sender privacy is ephemeral**: The value of knowing "who sent this" degrades over time as economic context becomes historical
3. **Amount hiding is quantum-safe**: Pedersen commitments have information-theoretic hiding

### The Deprecated LION Approach

LION (Lattice-based lInkable ring signatures fOr aNonymity) was a post-quantum ring signature scheme that would have provided quantum-safe sender privacy. It was deprecated because:

- **Size**: LION signatures are ~50x larger than CLSAG (~35 KB vs ~700 bytes per input)
- **Blockchain growth**: Would have made desktop nodes impractical
- **Limited benefit**: Sender privacy is ephemeral anyway

### What This Means for Users

- **No action required**: Your wallet works without any migration
- **Post-quantum protection**: Recipients are protected against "harvest now, decrypt later" attacks
- **Efficient transactions**: ~4 KB per private transaction instead of ~65 KB

For more details on Botho's cryptographic architecture, see:
- [Privacy documentation](privacy.md)
- [Architecture overview](architecture.md)
