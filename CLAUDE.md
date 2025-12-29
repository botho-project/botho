# Botho Project Guidelines

## Code Preservation

When removing or replacing code, prefer to **stash code rather than delete it outright**. This makes recovery easier if changes need to be reverted. Options include:

- Use `git stash` before making significant changes
- Comment out code blocks with a note explaining why (for temporary preservation)
- Create a backup branch before major refactors
- Use `git add -p` to stage changes incrementally

This is especially important for:
- Large refactors affecting multiple files
- Removing features that might be needed later
- Experimental changes during development

## Build Commands

- Build: `cargo build`
- Test: `cargo test`
- Run node: `cargo run --bin botho`

## Project Structure

This is a Rust-based blockchain project with the following key components:
- `botho/` - Main node binary
- `botho-wallet/` - Wallet implementation
- `blockchain/types/` - Core blockchain types
- `consensus/` - Consensus protocol (SCP-based)
- `transaction/` - Transaction handling
- `ledger/` - Ledger database
