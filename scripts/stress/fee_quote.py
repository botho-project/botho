"""Price signed drafts against the node's own fee floor (#2001).

Since protocol 10.0.0 a spend owes the class-exit charge from block 1
(#1685) and the import floor read from its inputs (#1687). Both depend on the
draft's ring members and on node-side cluster wealth, which the offline signer
cannot see, so its local size/cluster estimate underpays and the node refuses
the spend as ``Fee too low``.

The node quotes a signed draft with RPC ``estimateFee`` (``tx_hex``): its
``Ledger::consensus_fee_floor`` and the same floor at the relay congestion
base, plus the wallet settle/re-sign fees (``bth_cluster_tax::settle_fee`` /
``resign_fee``). ``sign_priced`` signs, quotes, and re-signs with the signer's
``min_fee`` at ``resignFee`` until the signed fee settles, exactly as
``botho send`` and the wallets do. Each underpaying draft is handed to
``discard`` (moved aside, never deleted, never submitted) before re-signing.
"""

from pathlib import Path

from runtime import Gate

# `botho_wallet::fee_estimation::MAX_FEE_PASSES`: every pass draws a fresh
# ring, so the floor moves between passes.
MAX_FEE_PASSES = 8

QUOTE_FIELDS = ("minimumFee", "consensusFeeFloor", "relayFeeFloor", "settleFee", "resignFee")


def check_quote(quote, fee):
    """Validate an ``estimateFee`` draft quote for a draft signed at ``fee``."""
    if not isinstance(quote, dict):
        raise Gate("invalid fee quote")
    for key in QUOTE_FIELDS:
        if type(quote.get(key)) is not int or quote[key] <= 0:
            raise Gate("invalid fee quote: " + key)
    if quote.get("txFee") != fee:
        raise Gate("fee quote is for a different draft")
    if not (quote["consensusFeeFloor"] <= quote["relayFeeFloor"] <= quote["minimumFee"]
            <= quote["settleFee"] <= quote["resignFee"]):
        raise Gate("inconsistent fee quote")
    return {key: quote[key] for key in QUOTE_FIELDS}


async def sign_priced(sign, quote, discard, artifact):
    """Sign until the fee settles against the node's quote.

    ``sign(min_fee)`` signs ``artifact`` (``min_fee`` 0 on the first pass,
    meaning: omit it) and returns the signer response; ``quote(tx_hex)``
    returns the node's ``estimateFee`` result for the draft; ``discard()``
    moves the underpaying draft out of ``artifact``. Returns the settled
    signer response with ``fee_floor_quote`` (the settling quote) and
    ``underpaid_drafts`` (hash, fee and quote of every discarded draft).
    """
    min_fee = 0
    drafts = []
    for _ in range(MAX_FEE_PASSES):
        info = await sign(min_fee)
        fee = info.get("fee")
        if type(fee) is not int or fee <= 0:
            raise Gate("missing/invalid signer fee")
        checked = check_quote(await quote(Path(artifact).read_bytes().hex()), fee)
        if fee >= checked["settleFee"]:
            return {**info, "fee_floor_quote": checked, "underpaid_drafts": drafts}
        drafts.append({"hash": info.get("hash"), "fee": fee, "quote": checked})
        discard()
        min_fee = max(checked["resignFee"], fee + 1)
    raise Gate("signed fee did not settle against the node fee floor after %d passes"
               % MAX_FEE_PASSES)
