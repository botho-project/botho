# Inactive CLSAG and arithmetic composition

This test-only library harness links ordinary public CLSAG ownership to the
same pseudo-output commitments used by the combined arithmetic experiment.
It is not a transaction format, production verifier, cryptographic security
proof, or accepted fee policy. Parent research: #1307; bounded child: #1346.

## What is linked

For input i, the real synthetic ring member commits to value V_i with blind
r_i. The arithmetic proof uses P_i = Com(V_i, p_i), with a different blind.
Ordinary `Clsag::sign` retains its built-in balance check, and `Clsag::verify`
receives exactly `inputs[i].amount_commitment` from the arithmetic statement.
The envelope has no independently supplied second P_i. The public verifier
receives no private key, opening, or real ring index. Fixtures use distinct
keys, amounts and blindings; each ring has 20 members and every charge is
nonzero. Canonical scalar bytes bridge the existing two Dalek versions; tests
check actual project generator and commitment equality.

The public research statement contains the arithmetic context, policy and
opaque fixture body digest, ordered input metadata, amount/charge/output
commitments, and every ordered ring member's public key, target key and
commitment. These are supplied fixture data, **not authenticated chain state**.
A field-coverage table and exhaustive struct destructuring check binding.
Hashing a typed statement is separate from validating its supported shape;
every proof entry point enforces the existing arithmetic audit and ring shape.

## Proof order and domains

The four domains are `botho/inactive/ownership-{statement,arithmetic,proof,message}-v1`.
They use labelled, length-framed Merlin messages and a 32-byte challenge for
each digest. This is an in-memory research transcript, not a wire codec.

1. Derive S from the complete public statement and ordered rings.
2. Prove the existing complete R1CS constraints under the separate arithmetic
   domain with S appended before commitments. Existing group conservation is
   still checked by verification.
3. Hash the exact resulting arithmetic proof bytes under the proof domain.
4. Derive each CLSAG message from S, that proof digest and its input index.
5. Recompute all digests from public data, verify R1CS, and verify every CLSAG
   signature against its arithmetic P_i. All components must pass.

There is no hash cycle: neither S nor the arithmetic proof contains the CLSAG
signatures or its own proof hash. The earlier arithmetic-only wrappers retain
their original transcripts and constraint/proof bodies. This child merely
adds explicit transcript-parameter helpers and a private module. The historical
[combined measurements](COMBINED.md) and their evidence file remain unchanged.

## Regression evidence

Four new tests cover real 1-input and 16-input round trips, each with ring20;
field/count/order binding; missing and extra signatures; reordered signatures;
a valid ordinary CLSAG signature against a different pseudo-output; and two
independently valid randomized arithmetic proofs. Replacing the first proof
with the second while retaining its signatures fails ownership verification;
re-signing the second proof's messages succeeds. Public-context and ring
controls separately reach arithmetic and ownership verification. No network
transaction or chain interaction is involved.

The dedicated signature-association helper is explicitly partial: only the
composed verifier checks both arithmetic and ownership. Shapes outside this
research configuration are rejected; this is not an external byte parser.

## Measured scope

One local release sample after compilation, with synthetic fixture generation
and generator setup outside the measured proof operations:

| Inputs / outputs | Ring | Arithmetic proof bytes | CLSAG field bytes | Composed generation | Composed verification |
|---|---|---|---|---|---|
| 1 / 1 | 20 | 1121 | 736 | 213.71 ms | 19.89 ms |
| 16 / 16 | 20 | 1377 | 11776 | 3174.43 ms | 302.49 ms |

Generator setup was 346.59 ms. Generation includes arithmetic proving, digest
work, OS seeding and ordinary CLSAG signing; verification includes public
validation, digest work, arithmetic verification and all CLSAG verification.
One sample is not a statistical benchmark or a latency budget, and these
composition timings are not standalone CLSAG benchmark measurements. The scalar
field payload count is 32 * (responses + initial challenge + two key images),
not a serialized signature or transaction size. Public statements, rings,
commitments, framing and other transaction components are not included. No
admission budget or memory claim is made.

Actual signing uses an OS-seeded advancing RNG. Synthetic fixture generation
is deterministic; actual arithmetic proofs retain upstream randomized proof
behavior. No crypto dependency version or randomness API was changed.

Reproduce the focused run:

```sh
RUSTC_WRAPPER= CARGO_TARGET_DIR=/tmp/botho-1346-target \
  cargo test --locked --release -p bth-ct-demurrage-reference ownership \
  -- --nocapture --test-threads=1
```

See `ownership-measurements.json` for source hashes, environment, exact command
and the captured focused output. The existing workspace CI already compiles
and executes this research crate in release; no broader CI suite was added.

## What remains outside this result

This demonstrates composition of existing public APIs on synthetic inputs.
It does not establish authenticated age, policy, lineage or ledger membership;
spentness/key-image uniqueness or consensus guarantees; transaction authorization
as a whole; privacy
across real ledger cohorts; replay or network admission rules; a reviewed
combiner security proof; deployment readiness; or CT integration. Existing
arithmetic cap/range/conservation controls are inherited, not a new proof of
all ownership semantics. Independent review remains necessary. No protocol,
codec, wallet, encrypted amount/memo, production signing, consensus or
activation code is changed.
