# Isolated sustained memory qualification

Issue #1452 contributes tools to #1437. A fixture passing these tools is not a
live qualification result, campaign acceptance, or recovery of #1440.

The read-only Linux sampler observes existing production processes. It does not
launch nodes, send payments, pause miners, modify a ledger, or access remote
hosts. A separately reviewed runner owns isolation, process launch, workload,
RPC observations, immutable deadlines, and teardown. Use dedicated qualification
hosts without Loom workers. Preserve historical incident hosts, markers, ledgers,
wallets and journals unchanged; do not reuse their signed intents or deadlines.

## Fixed profiles

The manifest's explicit phases are validated against these profiles. Changing a
deadline, phase, threshold or identity requires a new experiment, not a resumed
baseline. All samples carry the canonical manifest SHA256.

| Profile | Warmup | Sustained measurement | Follow-up | Total | Result scope |
|---|---|---|---|---|---|
| `minting` | 30 min | 4 h continuous minting | 30 s settling + 30 min verified idle | 5 h 30 s | `limited_minting` |
| `payments` | 30 min | 4 h sustained payments | 3 × (10 min active, 10 min idle) | 5.5 h | `sustained_payments` |

The first minting profile has **one** idle period. The production miner has no
pause/resume RPC, so the runner must arrange its normal pause condition at the
original 4.5-hour mint deadline. Do not invent three cycles, restart processes,
or suspend consensus with signals. Every declared producer must report
`mining=true` during warmup and measurement and `mining=false` during idle.
Relays must report `mining=false`. A fixed 30-second settling phase follows the deadline because the normal miner
control runs on a ten-second balance tick. Preserve all settling observations;
no mining-state gate applies during those thirty seconds. Every subsequent
idle observation must report mining disabled, with no further observed height advance. A missed deadline or
continued minting yields `incomplete`; never rebase the measurement window.

For payments, every fixed five-minute active window requires the predeclared
minimum block and confirmed-transfer increments on every node. Idle means no
confirmed-transfer increment after a fixed fifteen-second boundary allowance; mining can
continue. Confirmed transfers must come from canonical chain/RPC observations,
not submission or acceptance counts. The runner must verify this provenance.
The minting profile permits a transfer minimum of zero and cannot qualify
payment behavior or authorize a 72-hour campaign.

## Manifest and runner integration

Start with [the example manifest](../../scripts/stress/sustained-memory-manifest.example.json).
Replace every placeholder and list every node, including relays. Set a distinct
campaign and chain identifier, the reviewed source commit, and the exact binary
SHA256. The artifact-to-source correspondence is a runner/build provenance
obligation; the sampler verifies executable bytes, not the build history.

1. Start the isolated production processes in individual cgroup v2 units. Use
   explicit private peer membership and external network isolation. Separate
   local directories alone do not establish a distinct cryptographic network.
2. Verify all expected nodes are ready, connected to the intended cohort, and
   using the intended source artifact. For each process obtain its actual
   identity with `python3 scripts/stress/sample_sustained_memory.py --identity PID`.
   `process_start_id` is Linux boot ID plus `/proc/PID/stat` start ticks;
   `host_id` is `/etc/machine-id`. Copy `cgroup_path` into each node entry too;
   every node on a host must have a distinct process cgroup.
3. Capture `started_at_unix_s` once at the first measurable baseline after
   readiness, then write the manifest once. Align the runner's predeclared
   miner deadline to that baseline; readiness delays cannot shorten warmup.
   Use a five-second cadence and a maximum fifteen-second gap. Retain the
   original manifest, source/build evidence, launcher configuration and clock
   synchronization evidence. Never overwrite them with a later baseline.
4. For each node and tick, obtain fresh read-only RPC observations and write a
   local observation JSON file. It must contain:

   ```json
   {
     "observed_at_unix_s": 1790000000.0,
     "pid": 12345,
     "process_start_id": "BOOT_ID:START_TICKS",
     "chain_id": "ISOLATED_CAMPAIGN_CHAIN",
     "chain_height": 100,
     "chain_hash": "0000000000000000000000000000000000000000000000000000000000000064",
     "confirmed_transfers": 0,
     "mining": true,
     "errors": []
   }
   ```

   Bind `mining` to `minting_getStatus.active`. Bind tip height/hash to the
   same canonical chain response, not independent non-atomic RPC reads. Tie
   the endpoint to the intended process/chain before the experiment. An RPC
   timestamp must be no older than fifteen seconds, and cannot be in the
   future. Record RPC failures in `errors`; never synthesize success counters.

5. Collect one row, appending stdout to an append-only raw file:

   ```sh
   python3 scripts/stress/sample_sustained_memory.py \
     --manifest manifest.json --node n1 --observation n1-observation.json \
     >> observations.jsonl
   ```

   Collectors read local `/proc` and cgroups; a multi-host runner invokes one
   on each host and gathers their raw files without changing rows. Preserve
   per-node order. Do not use `set -e` to silently discard error rows: sampler
   exit 2 can include a valid JSONL row describing missing evidence. Retain
   stderr and exit status too. The sampler never substitutes zero for a
   failed read. Its CLI requires Linux cgroup v2 and Python 3.10 or newer.

6. Stop collection at the fixed profile end and enforce an independent hard
   six-hour lab deadline. Keep node processes alive through the idle period.
   Missing nodes, interrupted collection, restarts, failed idle transitions,
   or insufficient workload require a fresh experiment if retried. Preserve
   the incomplete evidence instead of extending its deadline.

7. Analyze after collection:

   ```sh
   python3 scripts/stress/sustained_memory.py manifest.json observations.jsonl \
     > assessment.json
   ```

   Analyzer exit codes: 0 `passed`, 1 `failed`, 2 `incomplete`. Check `scope`
   even on exit 0. Raw data is never rewritten. Archive the result together
   with raw observations, manifest and their hashes. Do not publish private
   operational artifacts automatically.

## Gate and interpretation

Every node must have complete coverage of the original warmup, measurement and
follow-up windows, with boundary error and inter-sample gaps at most fifteen
seconds. Counter regressions and PID/start changes make the run incomplete.
Source, artifact, host, cgroup, chain or manifest identity changes fail it. Malformed
or missing observations are incomplete, never implicit zeros or a pass.

For a complete node trace, the four-hour measurement uses ordinary least squares
over elapsed hours. Both **RSS + process swap** and **anonymous RSS + process
swap** must have slope ≤10 MiB/hour and end-minus-start growth ≤64 MiB.
The first measurement observation is the fixed memory baseline. Follow-up
endpoint growth also must remain ≤64 MiB against that same baseline. No moving
minimum, warmup reset, outlier deletion or later baseline selection is used.
Growth statistics from an incomplete four-hour measurement remain diagnostic.
A complete valid four-hour measurement that exceeds the growth limits fails
even if later idle coverage is missing; both findings remain in the report.
Any incomplete phase prevents a pass. A restart cannot turn a newly lower RSS
into a successful baseline.

RSS already contains anonymous RSS; do not add them together. Cgroup memory
and cgroup swap are separate aggregate diagnostics, never added to process
RSS. Explicit optional manifest limits `rss_plus_swap_limit_bytes` and
`cgroup_memory_plus_swap_limit_bytes` are absolute failure gates. Increases in
cgroup `oom` or `oom_kill` also fail even when collection is incomplete.
Resource failures take precedence over `incomplete`, whose reasons remain in
the report. Cgroup `high`/`max`, CPU time, throttling, and raw CPU/memory/I/O PSI
are retained for pressure diagnosis; pressure alone is not a configured memory
failure. Hashing each executable and collecting telemetry add observer overhead;
retain `sample_duration_s` and report the sampling setup with results.

The analyzer compares hashes only at identical chain identity and height.
Different-height tips are not evidence of a fork. A matching sampled overlap
does not prove absence of forks or complete consensus history. Five processes
on one host exercise production binaries but share hardware, kernel, caches and
failure domains; they do not establish equivalent dedicated-host performance.
State the actual topology and workload, never relabel a same-host minting pass
as full payment/campaign qualification.

## Offline checks

```sh
python3 -m unittest discover -s scripts/stress -p '*sustained_memory.py'
ruff check scripts/stress/*sustained_memory.py
ruff format --check scripts/stress/*sustained_memory.py
```

Synthetic controls include constant and warmed plateau traces, the historical
49 MiB/hour growth rate, swap-only growth, missing/restarted processes, altered
identities, insufficient workload, resource failures, phase deadlines and
same-height hash comparisons. Linux filesystem fixtures exercise the real
sampler's unit conversion, cgroup reads and process identity parsing. These
checks provide no live five-hour observation.
