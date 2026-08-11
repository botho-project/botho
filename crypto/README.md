## crypto

Provides implementations of cryptography primitives and wrappers around primitives needed in Botho components.

| Name    | Description |
| ------- | ----------- |
| [`box`](./box/README.md) | crypto_box style authenticated asymmetric key cryptography (Ristretto ECDH). |
| [`digestible`](./digestible/README.md) | Cryptographic hashing of structured data. |
| [`hashes`](./hashes/README.md) | Wrappers of cryptographic hash functions. |
| [`keys`](./keys/README.md) | Public key cryptography (Diffie-Hellman key exchange and digital signatures). |
| [`multisig`](./multisig/) | Multi-signature implementations. |
| [`pq`](./pq/) | Post-quantum cryptographic primitives (ML-KEM, ML-DSA). |
| [`ring-signature`](./ring-signature/README.md) | Amount commitments and MLSAG ring signatures, plus one-time keys. |
| [`secp256k1`](./secp256k1/) | Secp256k1 key support for Ethereum compatibility. |
| [`sig`](./sig/README.md) | API for schnorrkel digital signatures, using types from keys crate. |
