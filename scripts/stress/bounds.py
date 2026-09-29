"""Shared hard ceilings. Profile values may lower, never exceed, these bounds."""

LEGACY = {
    "max_unique_chain_writes": 800,
    "max_signed_fee_picocredits": 500_000_000_000,
    "max_single_signed_fee_picocredits": 5_000_000_000,
    "max_inflight": 8,
    "max_normal_submissions_per_minute": 4,
    "max_burst_size": 8,
    "max_rpc_per_endpoint_per_minute": 50,
    "max_queued_intents": 8,
    "start_slot_grace_seconds": 120,
    "max_signed_transaction_bytes": 262144,
    "max_wire_submission_bytes_per_minute": 2097152,
}
CEILINGS = {
    **LEGACY,
    "max_unique_chain_writes": 100000,
    "max_signed_fee_picocredits": 100_000_000_000_000,
    "max_inflight": 64,
    "max_normal_submissions_per_minute": 60,
    "max_burst_size": 64,
    "max_rpc_per_endpoint_per_minute": 600,
    "max_queued_intents": 128,
    "max_wire_submission_bytes_per_minute": 33554432,
}


def checked_limits(values):
    result = {}
    for key, ceiling in CEILINGS.items():
        value = values.get(key)
        if key.endswith("picocredits") and isinstance(value, str) and value.isdecimal():
            value = int(value)
        if type(value) is not int or not 1 <= value <= ceiling:
            raise ValueError("invalid bounded limit: " + key)
        result[key] = value
    if (
        result["max_single_signed_fee_picocredits"]
        > result["max_signed_fee_picocredits"]
    ):
        raise ValueError("inconsistent fee budgets")
    if values.get("max_automatic_rebroadcasts") != 0:
        raise ValueError("automatic rebroadcast disabled")
    return result
