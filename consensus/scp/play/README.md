## Intro

The `bth-consensus-scp-play` utility replays SCP debug-dump logs against a fake
local node. It is intended for debugging SCP-related panics or divergence by
re-running the exact message sequence a node observed.

## Status: no known log producer

**This tool currently has no producer in this repository.** It consumes the
`--scp-debug-dump` log format inherited from the upstream `consensus-service`
binary, which Botho does not carry: `--scp-debug-dump` / `MC_SCP_DEBUG_DUMP`
appear only inside this crate (`src/main.rs`), and the `botho` node binary does
not expose a flag to emit these logs.

The replay side is real and builds as a workspace member, so reviving the
workflow is a matter of teaching a Botho binary to write the dump — not of
rewriting this tool. Until then, treat the usage below as the input contract
rather than an end-to-end recipe.

## Usage

Given a directory of previously captured SCP debug-dump logs (one subdirectory
per node), replay one node's logs with:

```sh
MC_LOG=trace cargo run -p bth-consensus-scp-play -- --scp-debug-dump /tmp/scp/4
```

The dump path may also be supplied via the `MC_SCP_DEBUG_DUMP` environment
variable. `MC_LOG` is honored by Botho's logger as a fallback when `RUST_LOG` is
unset (`common/src/logger/loggers/mod.rs`), so `RUST_LOG=trace` works equally
well.
