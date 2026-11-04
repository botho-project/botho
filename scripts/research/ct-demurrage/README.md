# bth-ct-demurrage-reference

Inactive, test-only research crate: an exact-integer R1CS encoding of the CT
demurrage charge, plus randomized Bulletproofs prove/verify experiments. It is
not production proof code, consensus validation, or an accepted ADR. CI runs it
in `Workspace Build` (`cargo test --locked --release -p
bth-ct-demurrage-reference`).

## Which kernel this models

The circuit encodes the **pre-9.0.0 quantized** demurrage kernel, whose time
fraction is floored separately before the charge floor:

```text
time_fraction = elapsed * 1_000_000 / blocks_per_year
charge        = floor(value * rate * progressivity / 10_000 / 5_000)
                * time_fraction / 1_000_000
```

Protocol 9.0.0 (#1875, PR #1913) removes that time quantization from
production `bth_cluster_tax::demurrage_charge`. So the differential tests here
do **not** compare against production. They compare against
[`src/legacy_kernel.rs`](src/legacy_kernel.rs), a frozen copy of the
pre-9.0.0 `demurrage_charge` / `capitalized_reset_charge` /
`spend_demurrage_charge` taken from main at `326a87c8`. That copy is
intentionally not asserted equal to production.

The recorded resource measurements ([RESOURCES.md](RESOURCES.md),
[COMBINED.md](COMBINED.md), [OWNERSHIP.md](OWNERSHIP.md)) are for this
pre-9.0.0 circuit and are unchanged.

## Future work (#1914)

Re-derive the circuit for the exact 9.0.0 single-floor charge, re-record its
resource measurements, and restore a parity test against production
`bth_cluster_tax::demurrage_charge`.
