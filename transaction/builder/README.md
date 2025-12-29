# bth-transaction-builder

This crate provides the officially supported `TransactionBuilder` and related builder objects
that Botho clients use, such as `MemoBuilder` and `SignedContingentInputBuilder`.

This crate is not intended to become a part of the consensus enclave, instead
it is client oriented. It might be okay for it to require `std`, but currently it doesn't.

If you are looking to sign Botho transactions and you are working on an embedded device,
you might be looking for the `bth-crypto-ring-signature-signer` interface instead.
