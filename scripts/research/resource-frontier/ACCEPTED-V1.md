# Ordinary V1 accepted-block resource observations

Part of #1306. This is a bounded measurement of the current ordinary ledger's
validation and atomic application, not full CT1 cost or a fee recommendation.
The first local observations are retained under `evidence/accepted-v1-macos/`.
The existing illustrative cost report is unchanged. Hosted Linux evidence is
still required before acceptance.

The explicit ignored resource entry point lives beneath the existing
`tx_lifecycle_integration` test module and directly reuses its positive signed
payment, wallet recovery, ring20 selection and block assembly helpers. No
production path is replaced. Twenty funding/decoy blocks precede the measured
block; two sender coinbases provide independent inputs. Cases contain zero,
one or two one-input payments, with one recipient and one change output each.
Two fresh processes per case are observations, not a confidence interval or
throughput estimate. Coinbases and change are not counted as payments. The
existing fixture uses trivial test PoW and legacy V1 payment-output construction;
it is not the proposed CT1 or universal hybrid-output format.

Setup includes funding, signing, block assembly and initial metadata snapshots.
Only the call to `Ledger::add_block` is bracketed by process CPU and wall readings
(with the small measurement-call overhead). Postchecks verify tip, exact stored
block, fee/burn/pool conservation, output contents, key-image consumption,
transaction locations, recipient balances and close/reopen persistence outside
the measured interval. Failure stops the matrix; no retry or validation weakening.

CPU is user/system `getrusage(RUSAGE_SELF)` delta in microseconds, summed over all
threads in the fresh process. This is distinct from monotonic wall nanoseconds;
neither is electricity. The timeval representation has microsecond units, not
an assertion of actual microsecond accuracy. Zero quantized values are retained.
File sizes use Unix st_blocks × 512 separately from logical file length. Snapshot
values are taken before/after apply, not a storage-I/O trace. LMDB page reuse and
preallocation can yield zero growth despite new records.

Logical record accounting is intentionally a named subset: the measured block
row, its coinbase and payment-output UTXOs, and the two funding UTXOs, identified
in each sample. It excludes metadata, indexes, key images and other historical
rows. Encoded key/value byte totals match those rows' ordinary bincode/LE keys;
they are not total database size. Serialized block/transaction object lengths
are not network wire lengths, and omit transport, gossip replication and retries.
Randomized signatures and current fixture timestamps mean byte hashes need not
repeat across samples.

After source review, capture with:

```
python3 scripts/research/resource-frontier/measure_accepted_v1.py \
  --target-dir /path/to/approved-idle-target --output /new/evidence/directory
python3 scripts/research/resource-frontier/accepted_v1.py /new/evidence/directory
```

The collector compiles one release integration-test executable (15-minute bound),
selects it from Cargo JSON, and captures exactly six processes with 60-second
individual and 180-second matrix bounds. The exact ignored test is selected;
other ignored tests are not run. The output directory must be new. Source,
working-tree state, toolchain, executable and raw-log hashes are retained. The
manifest records the CPU model, hashed host identifier, relevant RUSTFLAGS and
CARGO_PROFILE_* overrides. Import rechecks the preserved Cargo JSON digest,
unique exact integration-test target/profile and each recorded invocation; a
binary digest is a captured identity, not an independent rehash of an absent
hosted executable.
Compile failure and incomplete/failed samples remain explicit outcomes.

The importer checks units, fields, source identity, raw logs, canonical distinct
outpoint identities, the two pre-apply and 4+2n post-apply selected rows, and complete accepted
denominators. It cannot infer a dishonest numeric CPU/wall swap from numbers;
collector provenance and review matter. Its output deliberately separates object
bytes, selected logical rows and file growth. Electricity, full wire bytes, CT1
CPU and a recommended BTH fee remain unknown, not zero. No readings are silently
inserted into the old illustrative model's differently defined fields.

Local host activity is not controlled; hosted Linux is not deployment calibration.
Operators still need real idle/load energy, tariffs, hardware, replication/role
overlap, traffic, storage policy and financing. The result cannot select an
arbitrary affordability fraction, ratify a 0.5 BTH target or forecast BTH value.
These observations can support a later partially measured cost frontier only
when those additional inputs and units are explicit.

## First local capture

Apple M3 Ultra, macOS/arm64, nightly-2025-12-03 release profile: opt-level 3,
debug assertions off, overflow checks on. Checkout `104aa463` plus the recorded
uncommitted source inventory; 32 exact source hashes bind the measured files.
A 15.12-second compile/build-script refresh produced executable SHA256
`7867ca0267cd74175c1d2ef6502bc7cb39b093542082e913c172ddd343366a40`.
This is the measured artifact, distinct from the preceding compile-only binary.
The preserved Cargo JSON and manifest identify the actual selected executable.

Six fresh processes all passed validation and reopen checks in 8.64 seconds total.
No other heavy work was initiated by this agent during capture; the host was not
isolated and this is not a calibrated throughput machine.

| Payments in block | Apply wall ms, samples 0/1 | Apply user+system CPU ms, samples 0/1 | Block object bytes | Transaction object bytes | Selected rows before→after | Allocated data-file delta bytes |
| --- | --- | --- | --- | --- | --- | --- |
|0|21.334 /26.199|17.704 /21.279|1533|none|2→4|0 /0|
|1|32.519 /30.897|28.357 /26.425|4412|2879|2→6|65536 /65536|
|2|34.074 /37.138|29.137 /32.387|7291|2879 each|2→8|65536 /65536|

These two observations per case are not quantiles. The empty-block cost contains
real ordinary PoW verification and ledger work and is not a payment cost. File
allocation jumps are page/allocation observations, not a marginal persistent
bytes-per-payment tariff. See the complete raw samples for separate user/system
CPU, setup, logical file length and the specifically identified encoded rows.
No energy, operator funding, network wire or CT1 cost is inferred.

Recheck the retained capture without executing Rust:

```
python3 scripts/research/resource-frontier/accepted_v1.py \
  scripts/research/resource-frontier/evidence/accepted-v1-macos
```

Historical captures must be checked against their matching source snapshot. This
macOS capture's 32 measured source files are present at commit
`64d2afc92eb97f0879e9fd3139f6563d796823b4`. The command above compares them with
the current checkout and is expected to reject later changes, including ledger
changes from #1357. Such a mismatch means the capture is stale for that checkout,
not that its raw observations are invalid. To review it later, use that matching
checkout, or call `load_evidence(evidence_directory, root=matching_checkout)`.
Never rehash an old measurement against new code to make the check pass.
