# Ring-member admission boundary

A valid CLSAG signature authenticates the supplied ring. It does not prove that
each supplied target key, public key and amount commitment corresponds to the
canonical ledger output. Admission and block application therefore use the same
`ValidationReads::verify_ring_members` comparison. The legacy target-key index
resolves its first outpoint; a later payout alias is not interchangeable with
that output when its amount commitment differs.

Admission rejects missing members and propagates lookup failures. Its subsequent
amount, tag and fee-input collection also fails if any member cannot be resolved;
it never drops a failed member while retaining the rest of the ring. Existing
signature, key-image, balance, fee and block checks remain in place. Corrupt or
dangling target-key indexes produce storage errors rather than absent outputs.

## Application admission and relay routes

The production call sites were audited with searches for `add_tx`,
`submit_transaction`, `register_transfer_tx`, `tx_relay` and
`broadcast_transaction` under `botho/src`:

| Route | Admission gate | Downstream use |
| --- | --- | --- |
| RPC `tx_submit` (`rpc/mod.rs`) | `Mempool::add_tx` | Only success sends to `tx_relay`; the run loop gossips and caches it |
| Faucet dispense and background settlement (`rpc/mod.rs`) | `Mempool::add_tx` | Accepted entries become available for mempool rebroadcast and nomination |
| Peer `NewTransaction` (`commands/run.rs`) | `Node::submit_transaction` → `Mempool::add_tx` | Only success calls `register_transfer_tx` and emits application notifications |
| Local node submission (`node/mod.rs`) | `Mempool::add_tx` | Mempool entry |
| Pending-file reload (`Node::load_pending_transactions`) | Re-submits every entry through `Node::submit_transaction` | Only accepted entries are returned for startup broadcast |
| Periodic re-announcement and transfer nomination (`commands/run.rs`) | Read previously accepted entries via `get_pending_transactions` | Gossip and `ConsensusService::submit_transaction` |

This is an application admission guarantee. libp2p gossipsub's transport signature
validation (`ValidationMode::Strict`) does not itself perform ledger validation;
this patch does not add application-controlled transport forwarding. Privacy
relay helpers likewise are not canonical ledger validators. Receipt of bytes on
the wire must not be interpreted as successful transaction admission.

## Direct SCP ingestion: separate assessment

`ConsensusService::submit_transaction` and `register_transfer_tx` accept serialized
bytes into the SCP cache without consulting the ledger. The production transfer
callers listed above gate those calls through mempool admission, but direct API
callers can bypass that prerequisite. `handle_message` processes SCP values that
reference cached transaction hashes; an uncached hash fails the cache lookup.
The validity callback calls `validate_from_bytes_intrinsic`, whose transfer check
is intentionally structural and independent of the local tip.

Consequently, this change does not claim that a direct cache insertion followed
by an SCP message is protected by admission. Adding the local ledger to that
callback would let honest nodes at different tips disagree on consensus value
validity. That is a separate protocol design question requiring an agreed state
boundary. Block application remains the canonical validation boundary even for
values that bypass the mempool; terminal handling of an externalized decision
that cannot be applied is tracked separately in #1440; recovery is downstream
of rejection and does not prevent such values from externalizing. This audit
establishes the bypass by call-site inspection, not a live malicious-peer or
multi-node SCP experiment. The separate prevention design tracked in #1447 should first reproduce
that route in a controlled SCP fixture, then specify validation against agreed
slot state and test nodes at different catch-up positions. Producer-side filtering
is tracked in #1443 and does not replace these checks.

## Regression evidence

`ledger/store/ring_admission_tests.rs` generates new synthetic keys, canonical
outputs and a later alias in a temporary LMDB ledger. The alias ring is signed
after construction, so rejection cannot be attributed to editing a signed
transaction. The tests exercise admission and actual block application, check
missing and mismatched members and corrupt storage, and preserve a canonical
signed control. They use no incident transactions, wallets or ledger artifacts.
