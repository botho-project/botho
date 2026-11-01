# DRT regression fixtures

One file per DRT leg, `<leg>.jsonl`, one JSON object per line with at least an
`input` field (the leg's input type, as serialized by `formal-drt/src/legs.rs`).
Every run replays every line of every selected leg before generating new cases.

The runner appends a line here, shrunk, whenever the Lean model and the Rust
code disagree (unless `--no-write-regressions`). Commit such a line together
with the fix, or with the bug issue it reproduces, so the case is replayed
from then on.

The lines committed with #1707 were found by the mutation check (see the
`note` field): each one disagrees with a mutated Rust function and agrees with
the real one. They pin the boundaries those mutations broke.
