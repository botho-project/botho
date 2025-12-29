# Botho Core Library (`bt-core`)

This crate provides (`no_std` and alloc free) core functionality required to support Botho wallets, including keys, addresses, and derivations (and in the future, ring signatures and transactions).

Types are defined in [`bt-core-types`](./types) for dependency loop avoidance.
Internal packages _should_ depend on `bt-core-types` unless functionality from `bt-core` is required, external packages _should_ depend on `bt-core` using re-exported types as internal arrangements may change.