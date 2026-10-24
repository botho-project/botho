# October 2026 certificate update

`rustls-webpki 0.103.16` fixes [RUSTSEC-2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382).
The workspace also pins `webpki-roots 1.0.10`; the remaining 0.26.11 wrapper
delegates to that updated bundle. Rebuild executables to incorporate these
statically linked dependencies. Updating a lockfile does not patch an
already-running process.

## Build and qualify

Use a clean checkout of the reviewed merge. Preserve unrelated working trees.
Record the commit, Cargo.lock SHA-256, `rustc --version --verbose`, build flags,
and output SHA-256. For the Linux arm64 faucet, the build environment
is the official Rust 1.98.1 Bookworm image at
`rust@sha256:5b993f23fb69746405496e76f91e511d96c82b5b23304fd5d20b4a80a8b223ea`.
Bookworm's glibc 2.36 is compatible with the faucet's newer Ubuntu userspace.
Select `RUSTUP_TOOLCHAIN=1.98.1` explicitly; `stable` can download a different
compiler. Leave `RUSTFLAGS` empty for the default portable arm64 target.

```bash
cargo test --locked -p botho-metrics-daemon
cargo clippy --locked -p botho-metrics-daemon --all-targets -- -D warnings
# In the recorded Linux build environment:
cargo build --locked --release -p botho-metrics-daemon
sha256sum target/release/botho-metrics-daemon
target/release/botho-metrics-daemon --version
```

Capture the live unit's user, working directory, executable path and database
path without printing its environment or credentials. Compare the running
executable's hash with the installed file. The historical source archive on
the faucet is older than its running binary and is not build provenance.

Take an online SQLite backup through the backup API from a read-only source
connection. Run the new executable against a **copy** of that backup in an
isolated network namespace, with `--node probe=http://127.0.0.1:9 --interval
300` and a test API port. Check `/health` and the history/latest APIs, then stop
the test process before an hourly rollup. Verify all existing table definitions
and rows remain intact. Creation of the optional empty `reserve_snapshots`
table is compatible with the previous daemon. A single-node legacy schema
requires a separate migration; do not delete a database to make this check pass.

## Replace only the metrics service

The operator-authorized target is `botho-metrics.service`, currently using
`/usr/local/bin/botho-metrics-daemon` and `/var/lib/botho-metrics/metrics.db`.
Confirm those paths from the live service before applying. Do not restart
Botho nodes, faucet RPC, campaign bots, or unrelated services.

1. Copy the qualified executable to a new staging directory, verify its exact
   recorded SHA and `--version` on the target, and capture the current unit,
   executable hash/owner/mode, process identity, `/health`, and API responses.
   Abort if the installed/running executable or service configuration changed
   since the reviewed plan.
2. As root, create a fresh mode-0700 directory under
   `/var/backups/botho-metrics/`. Save the old executable with its metadata and
   the unit's hash. Stop **only** `botho-metrics.service`, verify it stopped,
   and take a SQLite backup of the existing database into that directory.
   Require `PRAGMA integrity_check` to return `ok`; keep the original database
   in place with its current owner and mode.
3. Install the staged executable at a fresh temporary path in `/usr/local/bin`
   using the recorded owner/group/mode (currently root:root, 0755). Verify its
   SHA again, then atomically rename it onto `botho-metrics-daemon`. Start the
   existing service without rewriting its unit, arguments, or environment.
4. Verify active/running state, the new **running** executable hash, `/health`,
   history/latest API contracts, database integrity, and unchanged unit hash.
   Compare the saved database's existing rows with the live database; explain
   any normal collection/retention changes rather than assuming health means
   data preservation. Record old/new PIDs, hashes, backup paths, and health
   evidence. Pre-existing empty latest data or unreachable nodes remain
   separate operational findings.
5. If executable or health verification fails, stop only the metrics service,
   atomically restore the saved executable with its metadata, restart, and
   verify the old running hash and health. Retain the live database: the
   compatible optional table does not require restoring an older copy and
   discarding newly collected data. Keep the backup for independent recovery.

Database deletion instructions in the older first-install README do not apply
to this security update. Do not deploy the workspace's other executables as
part of the metrics replacement; active endurance campaigns continue running.
