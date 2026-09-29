# Adversarial fee cycles

Run `cargo test --locked -p botho --test adversarial_fee_cycles`. These four
non-ignored tests complement the live stress controller; they do not measure
network throughput or replace the 72-hour campaign. Workspace Build executes
this suite after its compile checks, with separate fifteen-minute compile and execution budgets. The package-scoped feature
graph can rebuild after workspace compilation; Linux debug signature checks
are substantially slower than local macOS runs.

The fixture uses production CLSAG signatures, persisted snapshot UTXOs, ledger
signature verification, consensus fee-floor calculation, mempool admission,
selection, removal, and dynamic fee state. Classical stealth outputs isolate
fee behavior from ML-KEM and decoy discovery. Seeds `1476`, `0xdeadbeef`, and
`0x7265636f76657279` reproduce all keys, outputs, ring permutations, and submission
orders. Failure messages identify the seed and cycle or constrained slot.

Each fixture has 32 UTXOs and at most 12 pending transactions. Tests cover:

- Both sides of every timing threshold, eight saturation/recovery cycles, fee
  deactivation outside the fastest timing tier, recovery while the fastest tier
  remains active, bounded diagnostics, and no residual fee drift.
- Shuffled 1x/2x/4x signed fee cohorts competing for three selection places.
  Equal shape and untagged outputs hold size and cluster factor constant. Every
  payment must appear exactly once, fees must be conserved, and removal must
  release key-image reservations. A forged high-fee offer must fail signature
  verification and admission before the valid cohort enters. This tests local
  candidate selection, not
  distributed consensus inclusion order.
- Default congestion quotes rising while the ordinary transaction's absolute
  `MIN_TX_FEE` still covers admission. A higher quote alone is not proof that a
  live campaign exercised a higher payable fee.
- Four rejection/recovery cycles under an explicitly elevated local relay base
  (10,000 pico/byte), forcing a boundary the default absolute floor masks.
  Rejections must be specifically `FeeTooLow`, leave no reservation, and become
  successful admissions after recovery. The identical signed transaction stays
  signature-valid and above the unchanged consensus fee floor throughout.

Timing windows supply synthetic timestamps and transaction counts to the
production timing function; they are not valid mined blocks and are never
applied to the ledger. Snapshot seeding bypasses minting and funding. Signature
and fee-floor checks do not claim full block application or SCP agreement.
The elevated relay policy is test-only and does not describe deployed defaults.
Live coverage must separately establish sustained backlog, actual validator
quotes and paid fees, inclusion latency, and recovery with real wallet inventory.
