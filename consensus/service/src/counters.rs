// Copyright (c) 2018-2022 The MobileCoin Foundation

use mc_util_metrics::{
    register, register_histogram, Collector, Desc, Histogram, IntCounter, IntCounterVec, IntGauge,
    MetricFamily, OpMetrics, Opts,
};
use std::sync::LazyLock;

pub static OP_COUNTERS: LazyLock<OpMetrics> =
    LazyLock::new(|| OpMetrics::new_and_registered("consensus_scp"));

pub static TX_VALIDATION_ERROR_COUNTER: LazyLock<TxValidationErrorMetrics> =
    LazyLock::new(TxValidationErrorMetrics::new_and_registered);

pub static PENDING_VALUE_PROCESSING_TIME: LazyLock<Histogram> = LazyLock::new(|| {
    register_histogram!(
        "pending_value_processing_time",
        "Time from receiving a value until it is externalized (in seconds)",
        vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 15.0, 20.0, 25.0, 30.0, 45.0, 60.0
        ]
    )
    .unwrap()
});

// consensus_msgs_from_network queue size.
pub static CONSENSUS_MSGS_FROM_NETWORK_QUEUE_SIZE: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("consensus_msgs_from_network_queue_size"));

// Transactions externalized through byzantine ledger service since this node started.
pub static TX_EXTERNALIZED_COUNT: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("tx_externalized_count"));

// Number of pending values. NOTE: This gauge is also used to rate limit
// add_transaction requests, in order that our metered_channel of pending values remains
// under a set limit.
pub static CUR_NUM_PENDING_VALUES: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("cur_num_pending_values"));

// Current slot number.
pub static CUR_SLOT_NUM: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("cur_slot_num"));

// Current slot phase.
pub static CUR_SLOT_PHASE: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("cur_slot_phase"));

// Current slot number of voted nominated values.
pub static CUR_SLOT_NUM_VOTED_NOMINATED: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("cur_slot_num_voted_nominated"));

// Current slot number of accepted nominated values.
pub static CUR_SLOT_NUM_ACCEPTED_NOMINATED: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("cur_slot_num_accepted_nominated"));

// Current slot number of confirmed nominated values.
pub static CUR_SLOT_NUM_CONFIRMED_NOMINATED: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("cur_slot_num_confirmed_nominated"));

// Current slot nomination round.
pub static CUR_SLOT_NOMINATION_ROUND: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("cur_slot_nomination_round"));

// Current slot ballot counter.
pub static CUR_SLOT_BALLOT_COUNTER: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("cur_slot_ballot_counter"));

// Previous slot number.
pub static PREV_SLOT_NUMBER: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("prev_slot_number"));

// Timestamp of when the last slot has ended.
pub static PREV_SLOT_ENDED_AT: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("prev_slot_ended_at"));

// Number of values externalized in the previous slot.
pub static PREV_SLOT_NUM_EXT_VALS: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("prev_slot_num_ext_vals"));

// Number of pending values when the previous slot ended.
pub static PREV_NUM_PENDING_VALUES: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("prev_num_pending_values"));

// Previous slot number of voted nominated values.
pub static PREV_SLOT_NUM_VOTED_NOMINATED: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("prev_slot_num_voted_nominated"));

// Previous slot number of accepted nominated values.
pub static PREV_SLOT_NUM_ACCEPTED_NOMINATED: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("prev_slot_num_accepted_nominated"));

// Previous slot number of confirmed nominated values.
pub static PREV_SLOT_NUM_CONFIRMED_NOMINATED: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("prev_slot_num_confirmed_nominated"));

// Previous slot nomination round.
pub static PREV_SLOT_NOMINATION_ROUND: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("prev_slot_nomination_round"));

// ByzantineLedger message queue size.
pub static BYZANTINE_LEDGER_MESSAGE_QUEUE_SIZE: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("byzantine_ledger_msg_queue_size"));

// Number of entries in the transactions cache.
pub static TX_CACHE_NUM_ENTRIES: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("tx_cache_num_entries"));

// Number of consensus messages dropped due to referencing an invalid previous block id.
pub static SCP_MESSAGES_DROPPED_DUE_TO_INVALID_PREV_BLOCK_ID: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("scp_messages_dropped_due_to_invalid_prev_block_id"));

// Number of times catchup is initiated
pub static CATCHUP_INITIATED: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("catchup_initiated"));

// Number of times attestation is initiated
pub static ATTESTATION_INITIATED: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("attestation_initiated"));

// Number of times a transaction is added to the user_api_service
pub static ADD_TX_INITIATED: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("add_tx_initiated"));

// Number of times a transaction is added to the user_api_service
pub static ADD_TX: LazyLock<IntCounter> = LazyLock::new(|| OP_COUNTERS.counter("add_tx"));

// Time it takes to perform the well-formed check
pub static WELL_FORMED_CHECK_TIME: LazyLock<Histogram> =
    LazyLock::new(|| OP_COUNTERS.histogram("well_formed_check_time"));

// Time it takes to validate a transaction
pub static VALIDATE_TX_TIME: LazyLock<Histogram> =
    LazyLock::new(|| OP_COUNTERS.histogram("validate_tx_time"));

// Consensus enclave attestation evidence timestamp, represented as seconds of UTC time since Unix epoch 1970-01-01T00:00:00Z.
pub static ENCLAVE_ATTESTATION_EVIDENCE_TIMESTAMP: LazyLock<IntGauge> =
    LazyLock::new(|| OP_COUNTERS.gauge("enclave_attestation_evidence_timestamp"));

// Number of times a ProposeMintConfigTx call has been initiated.
pub static PROPOSE_MINT_CONFIG_TX_INITIATED: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("propose_mint_config_tx_initiated"));

// Number of times a ProposeMintConfigTx call has returned a response.
pub static PROPOSE_MINT_CONFIG_TX: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("propose_mint_config_tx"));

// Number of times a ProposeMintTx call has been initiated.
pub static PROPOSE_MINT_TX_INITIATED: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("propose_mint_tx_initiated"));

// Number of times a ProposeMintTx call has returned a response.
pub static PROPOSE_MINT_TX: LazyLock<IntCounter> =
    LazyLock::new(|| OP_COUNTERS.counter("propose_mint_tx"));

/// TxValidationErrorMetrics keeps track of tx validation errors upon ingress
/// (with the add tx GRPC call). This cannot use the standard OpMetrics since we
/// want to have a separate counter per each error (OpMetrics uses an "op" label
/// to distinguish between different counters, this metric will have an "err"
/// label).
#[derive(Clone)]
pub struct TxValidationErrorMetrics {
    counters: IntCounterVec,
}

impl TxValidationErrorMetrics {
    pub fn new() -> Self {
        Self {
            counters: IntCounterVec::new(
                Opts::new(
                    "consensus_service_tx_validation_errors",
                    "Counters for consensus service transaction validation errors",
                ),
                &["err"],
            )
            .unwrap(),
        }
    }

    pub fn new_and_registered() -> Self {
        let metrics = Self::new();
        register(Box::new(metrics.clone()))
            .expect("TxValidationErrorMetrics registration on Prometheus failed.");

        metrics
    }

    pub fn inc(&self, err: &str) {
        self.counters.with_label_values(&[err]).inc();
    }
}

impl Collector for TxValidationErrorMetrics {
    fn desc(&self) -> Vec<&Desc> {
        self.counters.desc()
    }
    fn collect(&self) -> Vec<MetricFamily> {
        self.counters.collect()
    }
}
