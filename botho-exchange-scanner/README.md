## botho-exchange-scanner

A client-side deposit detection tool for exchanges integrating with the Botho
blockchain. It scans chain outputs against precomputed subaddress lookup tables
to detect deposits, with resumable sync state.

### Key modules (`src/`)

- `scanner` — the core output-scanning engine.
- `subaddress` — precomputed subaddress lookup tables (0 to 2^64 range).
- `deposit` — detected-deposit representation.
- `output` — output parsing/handling.
- `sync` — sync-state persistence for resumable scanning.
- `config` — scanner configuration (`main.rs` is the CLI entry point).

### Workspace fit

A standalone integration binary. It consumes the workspace's transaction and
crypto primitives to recognize outputs belonging to an exchange's subaddress
ranges, without running a full node.
