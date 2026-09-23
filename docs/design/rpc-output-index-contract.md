# RPC output identity and derivation index

`chain_getOutputs` retains its historical `txHash` and `outputIndex` fields.
These identify an RPC record; they are not universally the ledger outpoint or
cryptographic derivation index. No consensus or historical block encoding changes.

| Output | Legacy RPC identifier | Additive `ledgerOutpoint` | Additive `cryptoOutputIndex` |
| --- | --- | --- | --- |
| Coinbase (`coinbase: true`) | minting transaction hash, `4294967295` | block hash, `0` | `0` |
| Ordinary transaction | transaction hash, output ordinal | same tuple | same ordinal |
| Legacy lottery (`lottery: true`) | block hash, `1 + payout ordinal` | same tuple | omitted |

`ledgerOutpoint` is an object with `txHash` and `outputIndex`. It is useful for
canonical ledger lookup; it must never replace the derivation index. The native
thin wallet retains legacy identifiers in its owned-output cache and carries the
optional crypto index separately through scanning, key-image recovery and signing.
The existing node wallet reads canonical ledger coinbase index zero already.

For older nodes, a response with the existing `coinbase: true` discriminator and
legacy MAX index resolves to crypto index zero even without the additive fields.
A bare MAX without that discriminator is ambiguous and is not normalized. Older
serialized native caches remain readable, but a MAX record without the discriminator
requires a fresh scan. Ordinary cached outputs without additive fields retain their
original indices. Conflicting explicit crypto indices are rejected during recovery
and skipped with a warning during scanning, rather than changing record identity.

Web remote, Snap, snap-spike and mobile currently normalize the historical MAX
sentinel to zero at their scan boundary. Their inputs remain compatible because
the existing RPC fields and discriminator are unchanged. The WASM signer continues
to consume the crypto index provided by its adapter. New additive fields do not
require old clients to change their decoding. There is no cache-ID rewrite or
canonical-ID migration in this change.

Legacy lottery records retain their previous identity and scanning behavior.
Their payout ordinal is not a claim about the source's original crypto index.
This change does not repair legacy payout ownership or activate the inactive V2
context path tracked by #1286. All RPC metadata has the existing node-trust model;
these fields are not authenticated header proofs.

## Positive regression

`botho/tests/hybrid_coinbase_rpc.rs` generates a temporary accepted chain, directly
mints one hybrid reward to the thin wallet, and uses the production loopback RPC
response and native decoder/scanner. It verifies all three identity/index contracts,
old-node additive-field absence, ambiguous old-cache rescan, and stable legacy IDs.
It then restores keys/cache, uses the production thin-wallet signer and RPC-decoded
decoy pool in the existing age window, submits through the production mempool, and
accepts the transaction in the ledger. Actual key-image spent queries filter the
spent reward. A second accepted spend from hybrid change at index one verifies
that ordinary nonzero indices stay intact. Recipient, fees and change reconcile.

The fixture's trivial PoW and temporary loopback server follow existing positive
ledger/RPC fixtures. It does not contact the public network, spend live funds,
change a deployment or reproduce adversarial signed transactions.
