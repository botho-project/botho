## bth-address-codec

The single shared base58 codec for Botho address strings (address format v2,
ADR 0008).

### Role

Historically the `botho://…` address string was encoded and decoded by four
independent hand-rolled base58 implementations (node, browser wasm-signer,
mobile FFI, and the CLI wallet / desktop shell), which risked byte-for-byte
drift between encoders. This crate consolidates that into one implementation so
every encoder agrees on the wire format.

### Key API (`src/lib.rs`)

- `encode_address(&PublicAddress, Network) -> String` — encode an address.
- `decode_address(&str) -> (PublicAddress, Network)` — decode and validate.
- `Network::v2_prefix` — the per-network address string prefix.

### Workspace fit

A small foundational crate depended on by every component that needs to render
or parse Botho address strings (node, wallet, mobile, wasm-signer), keeping them
from drifting.
