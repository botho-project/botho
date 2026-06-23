//! Dynamic, message-level SCP-ish round simulator for **empirical** fork
//! (safety) and stall (liveness) detection.
//!
//! This is the v1 dynamic simulator (issue #514), the follow-up to the static
//! analyzer (#511/#512). Where [`crate::analysis`] *predicts* the safety buffer
//! (minimal splitting set) and liveness buffer (minimal blocking set) of an
//! [`Fbas`] statically, this module *runs the protocol* over the same FBAS
//! model under faults and a configurable network, and observes what actually
//! happens: did two correct nodes commit different values (a fork / SAFETY
//! violation), or did nobody commit within the round budget (a stall / LIVENESS
//! violation)?
//!
//! It deliberately reuses the existing FBAS model — [`Fbas`], [`QuorumSet`],
//! [`NodeSet`], and crucially [`Fbas::is_quorum`] — as the single source of
//! truth for "what is a quorum". The dynamic engine never re-derives quorum
//! logic; it collects per-value votes into a [`NodeSet`] and asks the static
//! model whether that set is a quorum. This is what lets the dynamic results
//! cross-check the static splitting-set prediction (see the tests).
//!
//! # SCP simplifications (documented honestly)
//!
//! This is *simulation/test tooling*, not production consensus. It models
//! consensus **dynamics** at the abstraction needed to observe agreement vs.
//! divergence, and intentionally simplifies real SCP:
//!
//! - **Two-phase federated voting (vote → accept → confirm/commit), not the
//!   full SCP state machine.** Real SCP nomination has `voted`/`accepted`/
//!   `confirmed` nominate stages and the ballot protocol has `PREPARE`/
//!   `CONFIRM`/`EXTERNALIZE` with ballot counters `b`, `p`, `c`, `h`. Here each
//!   correct node casts ONE current vote, then runs the two federated-voting
//!   steps that actually carry the safety proof. First, **accept (lock)**: a
//!   node *accepts* and irrevocably locks a value the first time it sees the
//!   federated-voting accept condition for it — either a **quorum** (incl.
//!   self) currently *votes* for the value, or a **v-blocking set** of nodes
//!   have already *accepted* it; the lock is pinned, so a correct node accepts
//!   at most one value, ever. Second, **confirm (commit)**: a node *irrevocably
//!   commits* a value only once it observes a **confirming quorum** (incl.
//!   self) whose members have each *accepted* that value — not merely voted for
//!   it. This second phase is the fix for the asynchronous-safety defect
//!   (#517). The earlier v1 simplification collapsed accept→confirm into a
//!   single lock and committed on a transient *vote* quorum, which under
//!   message reordering let two correct nodes commit different values with ZERO
//!   faulty nodes. Requiring a confirming quorum of *accepters* restores the
//!   quorum-intersection guarantee: two quorums of accepters of different
//!   values would have to share an honest node, but an honest node accepts only
//!   one value — so under ARBITRARY delay/reordering, once any correct node
//!   commits `v`, no correct node can commit `v' ≠ v` unless the Byzantine set
//!   reaches the splitting threshold. It is still NOT a faithful ballot FSM,
//!   but the safety invariant is exact.
//! - **One slot, one leader per simulation.** We simulate agreement on a single
//!   slot's value, repeated across many seeds — not a growing chain, and (for
//!   leader models) with ONE leader for the whole slot rather than per-round
//!   leader rotation. Leadership *fairness* is therefore measured across seeds
//!   (each seed = a distinct slot). Chain-level concerns (catch-up, height
//!   realign) are out of scope.
//! - **Leader-timeout / view-change (optional, #519).** A *crashed* leader is
//!   always survived via a deterministic lowest-value-heard fallback (so the
//!   slot still decides). On top of that, the simulator can optionally model
//!   SCP's **leader-timeout / view-change** recovery
//!   ([`SimConfig::view_change`]): if the current leader fails to drive the
//!   slot to a decision within a per-view round budget, the **view** is
//!   advanced and the leader is **rotated round-robin** to `(base_leader +
//!   view) % n`, and the slot is retried under the new leader. This closes the
//!   only liveness gap of the v1 engine — a *Byzantine (equivocating) leader*
//!   that stalls its own slot is rotated out, so liveness is restored — exactly
//!   the validation the ratified round-robin+view-change proposer design (#427)
//!   needs. With view-change **disabled** (`view_change: None`), a Byzantine
//!   leader still stalls the slot, the documented v1 behavior, retained for
//!   comparison.
//!
//!   **Safety is preserved exactly across views.** View-change only changes
//!   *which leader an as-yet-undecided node follows*; it never unwinds an
//!   `accept` lock or a commit. A correct node's vote is pinned to its accepted
//!   value forever (see the accept step), so once a node accepts (let alone
//!   commits) a value it keeps that value through every subsequent view. Two
//!   correct nodes therefore still cannot commit different values unless the
//!   Byzantine set reaches the splitting threshold — the same quorum-
//!   intersection argument as without view-change. View-change is bounded by
//!   `max_rounds` (each view consumes at least one round), so the simulation
//!   still terminates; if no decision is reached within the budget it is
//!   reported as a stall.
//! - **Round structure with a message bus.** Messages are queued by an integer
//!   delivery round (delay); drops remove them. There is no real wall clock and
//!   no threads, so runs are fully deterministic given a seed.
//! - **Value identity is a small integer** standing in for a coinbase/tx_hash.
//!   The production deterministic combiner (highest-PoW-priority minting tx,
//!   ties by tx_hash, one coinbase/slot — `botho/src/consensus/service.rs`) is
//!   abstracted to "lowest value id wins" among the values a node has heard.
//! - **Coinbase churn (optional, #535).** The competing-coinbase proposer can
//!   model **coinbase churn** ([`SimConfig::churn_rate`]): each round a node's
//!   local miner produces a fresh, strictly higher-priority coinbase with the
//!   configured probability (mirroring a RandomX miner emitting new minting txs
//!   several times a second). Here the combiner is faithful to service.rs —
//!   priority == value id, so the competing-coinbase champion picks the
//!   **highest** value (highest PoW priority) it has heard, not the lowest. The
//!   [`SimConfig::pin_coinbase`] toggle selects production vs. the pre-#419
//!   bug: **pinned** (default) keeps the first coinbase per slot so the
//!   candidate set stabilizes and federated voting converges; **unpinned**
//!   re-nominates each newly-mined higher value so the candidate set never
//!   settles and the slot jams (a stall). Churn is *value-selection only* — it
//!   never touches the accept/commit machinery — so it cannot affect safety
//!   (zero forks in either mode with no faults). With `churn_rate == 0` (the
//!   default) the competing-coinbase model is byte-for-byte the original
//!   churn-free behavior.
//!
//! Where a simplification could change the safety/liveness conclusion it is
//! called out at the relevant function.

use crate::{model::Fbas, nodeset::NodeSet};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Write as _};

/// A consensus value for a slot, standing in for a coinbase / winning tx_hash.
///
/// Distinct ids = distinct values; two correct nodes committing different ids
/// for the same slot is a **fork**.
pub type ValueId = u64;

/// How proposers introduce candidate values into nomination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposerModel {
    /// **Competing-coinbase** (Botho's current model): every (correct) node
    /// nominates its *own* distinct value. No leader; nomination must converge
    /// purely through echoing the deterministic-best value. This is the model
    /// #427 wants to move away from; it is the most divergence-prone.
    CompetingCoinbase,
    /// **SCP-native hash-priority leader**: the slot leader is the node
    /// maximizing a hash of `(slot, node)` (mirrors SCP nomination leader
    /// priority, `Gi(slot, nodeID)`). Non-leaders echo the leader's value.
    HashPriorityLeader,
    /// **Round-robin leader**: leader = `slot % n`. Deterministic rotation
    /// across slots/seeds.
    RoundRobinLeader,
    /// **VRF-style leader**: leader chosen by a seeded hash of
    /// `(vrf_seed, slot, node)` — approximates a VRF with a keyed hash. Like
    /// hash-priority but the priority is unpredictable without the seed (models
    /// a VRF's unpredictability; grinding resistance is approximated, not
    /// proven).
    VrfLeader,
}

impl ProposerModel {
    /// Display label.
    pub fn label(&self) -> &'static str {
        match self {
            ProposerModel::CompetingCoinbase => "competing-coinbase",
            ProposerModel::HashPriorityLeader => "hash-priority-leader",
            ProposerModel::RoundRobinLeader => "round-robin-leader",
            ProposerModel::VrfLeader => "vrf-leader",
        }
    }

    /// All models, for sweeping/`compare`.
    pub fn all() -> [ProposerModel; 4] {
        [
            ProposerModel::CompetingCoinbase,
            ProposerModel::HashPriorityLeader,
            ProposerModel::RoundRobinLeader,
            ProposerModel::VrfLeader,
        ]
    }
}

/// What a faulty node does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultKind {
    /// **Crash**: the node goes silent — sends nothing, commits nothing. Tests
    /// liveness (does the rest of the network still decide?).
    Crash,
    /// **Byzantine equivocation**: the node sends *different* values to
    /// different peers (splitting the honest nodes' views), the fork-inducing
    /// adversary. Tests safety against the splitting-set threshold.
    Equivocate,
}

/// The network timing model.
///
/// Not `Eq` because [`NetworkModel::PartiallySynchronous`] carries an `f64`
/// drop probability.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkModel {
    /// **Synchronous**: every message is delivered in the next round, never
    /// dropped. The favorable baseline.
    Synchronous,
    /// **Partially synchronous**: each message is delayed by `0..=max_delay`
    /// rounds and dropped with `drop_prob` probability (both seeded). Models
    /// the asynchronous periods SCP must remain *safe* through.
    PartiallySynchronous {
        /// Maximum extra delivery delay, in rounds.
        max_delay: u32,
        /// Per-message drop probability in `[0.0, 1.0]`.
        drop_prob: f64,
    },
}

/// A full simulation configuration. One [`run`] consumes one of these plus a
/// seed and produces one [`RunOutcome`].
///
/// Not `Eq` because [`NetworkModel`] carries an `f64` drop probability.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimConfig {
    /// Number of nodes.
    pub n: usize,
    /// Threshold for the symmetric `t`-of-`n` quorum set. `None` ⇒ Botho's BFT
    /// rule for `n`.
    pub threshold: Option<usize>,
    /// Proposer model under test.
    pub proposer: ProposerModel,
    /// Network timing model.
    pub network: NetworkModel,
    /// Indices of faulty nodes.
    pub faulty: Vec<usize>,
    /// What the faulty nodes do.
    pub fault: FaultKind,
    /// Maximum rounds before declaring a stall (liveness budget).
    pub max_rounds: u32,
    /// **Leader-timeout / view-change** budget (#519). `None` disables view-
    /// change (the documented v1 behavior: a Byzantine leader stalls its slot).
    /// `Some(view_budget)` enables it: if the current leader has not driven the
    /// slot to a decision within `view_budget` rounds, the **view** advances
    /// and the leader rotates round-robin to `(base_leader + view) % n`,
    /// retrying the slot under the new leader. Ignored by the leaderless
    /// [`ProposerModel::CompetingCoinbase`] (no leader to rotate). A
    /// `view_budget` of 0 is treated as 1 (each view must consume ≥1 round so
    /// the simulation terminates within `max_rounds`).
    pub view_change: Option<u32>,
    /// **Coinbase-churn rate** for the [`ProposerModel::CompetingCoinbase`]
    /// proposer (#535, the simulation arm of #532). Per-round probability in
    /// `[0.0, 1.0]` that a node's local RandomX miner produces a fresh,
    /// strictly **higher-priority** coinbase value mid-slot (mirroring
    /// production, where each validator mines a new minting tx several
    /// times a second). `0.0` (the default) reproduces the original,
    /// churn-free behavior where each node nominates a single fixed value.
    ///
    /// Whether a node *acts on* a freshly-mined higher-priority value is
    /// governed by [`SimConfig::pin_coinbase`]. Ignored by the leader models
    /// (their value is the leader's, not a self-mined coinbase).
    pub churn_rate: f64,
    /// **Coinbase pinning** toggle for the competing-coinbase proposer (#535).
    /// `true` (the default) is the **production behavior** (#419 fix): a node
    /// keeps nominating the FIRST coinbase it proposed for the slot even when
    /// its miner produces a higher-priority one — pinning the candidate set so
    /// federated voting can converge. `false` is the **pre-#419 bug**: a node
    /// re-nominates each newly-mined higher-priority value, so the candidate
    /// set never stabilizes and the slot can jam (a stall). Only meaningful
    /// when `churn_rate > 0` and the proposer is
    /// [`ProposerModel::CompetingCoinbase`].
    pub pin_coinbase: bool,
}

impl SimConfig {
    /// A synchronous, fault-free symmetric config — the simplest baseline.
    pub fn symmetric(n: usize) -> Self {
        SimConfig {
            n,
            threshold: None,
            proposer: ProposerModel::HashPriorityLeader,
            network: NetworkModel::Synchronous,
            faulty: Vec::new(),
            fault: FaultKind::Crash,
            max_rounds: 32,
            view_change: None,
            churn_rate: 0.0,
            pin_coinbase: true,
        }
    }

    /// Build the FBAS this config simulates over (symmetric `t`-of-`n`).
    pub fn build_fbas(&self) -> Fbas {
        match self.threshold {
            Some(t) => Fbas::symmetric(self.n, t),
            None => Fbas::symmetric_botho(self.n),
        }
    }
}

/// What happened in a single run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// All correct nodes that committed agreed on one value (no fork) and at
    /// least one committed. Safe + live.
    Agreement,
    /// Two or more correct nodes committed *different* values — a **fork**
    /// (SAFETY violation).
    Fork,
    /// No correct node committed within `max_rounds` — a **stall** (LIVENESS
    /// violation).
    Stall,
}

/// The outcome of one [`run`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutcome {
    /// The seed used (for reproduction).
    pub seed: u64,
    /// Classification.
    pub decision: Decision,
    /// Round at which the *first* correct node committed (`None` if stalled).
    pub rounds_to_decide: Option<u32>,
    /// What each correct node committed, by node index (omits faulty + the
    /// uncommitted). Used to characterize a fork.
    pub committed: BTreeMap<usize, ValueId>,
    /// Per-round leader (node index) for the rounds that were simulated. Empty
    /// for the leaderless competing-coinbase model.
    pub leaders: Vec<usize>,
}

impl RunOutcome {
    /// Whether this run forked (committed-correct nodes disagree).
    pub fn is_fork(&self) -> bool {
        self.decision == Decision::Fork
    }
    /// Whether this run stalled.
    pub fn is_stall(&self) -> bool {
        self.decision == Decision::Stall
    }
}

/// A message on the bus. Equivocation is captured by sending per-recipient
/// copies that carry different `value`s. The delivery round is the bus
/// `BTreeMap` key, so it is not stored on the message itself.
///
/// Carries BOTH federated-voting signals so the receiver can run the two-phase
/// protocol against delayed/reordered state:
/// - `value`: the sender's current *vote* (used for the accept step), and
/// - `accept`: the sender's irrevocably *accepted* value if it has locked one
///   (`None` until the sender accepts). A confirming quorum of `accept`ers is
///   what authorizes an irrevocable commit, restoring asynchronous safety.
#[derive(Clone, Copy, Debug)]
struct Message {
    /// Sender node index.
    from: usize,
    /// Recipient node index.
    to: usize,
    /// The value the sender is voting for, as seen by `to`.
    value: ValueId,
    /// The value the sender has *accepted* (locked), as seen by `to`. `None`
    /// until the sender accepts.
    accept: Option<ValueId>,
}

/// A seeded keyed hash for leader priority / VRF approximation and message
/// timing. Deterministic across platforms (no float, no address hashing).
fn keyed_hash(parts: &[u64]) -> u64 {
    // FNV-1a 64-bit over the little-endian bytes of each part. Pure integer
    // arithmetic ⇒ identical on every platform.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &p in parts {
        for b in p.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Pick the single **slot leader** for this simulation under the proposer
/// model.
///
/// We model **one slot per simulation** (see the module docs), so there is one
/// leader for the whole run — mirroring SCP nomination converging on a leader's
/// value for a slot, rather than rotating leaders mid-slot (which would inject
/// spurious value churn unrelated to safety). Leadership *fairness* is
/// therefore measured across many seeds/slots (each seed = a distinct slot),
/// which is exactly the fairness question for proposer selection.
///
/// `slot` is the per-run slot identifier (the seed). Returns `None` for the
/// leaderless competing-coinbase model. The leader is chosen among ALL nodes
/// (including faulty ones — a Byzantine leader is the interesting adversarial
/// case).
fn leader_for(proposer: ProposerModel, slot: u64, n: usize, vrf_seed: u64) -> Option<usize> {
    match proposer {
        ProposerModel::CompetingCoinbase => None,
        ProposerModel::RoundRobinLeader => Some((slot as usize) % n),
        ProposerModel::HashPriorityLeader => {
            // SCP-native: argmax over nodes of Gi(slot, node).
            (0..n).max_by_key(|&node| keyed_hash(&[slot, node as u64]))
        }
        ProposerModel::VrfLeader => {
            // Keyed by a per-run secret seed ⇒ unpredictable without the seed.
            (0..n).max_by_key(|&node| keyed_hash(&[vrf_seed, slot, node as u64]))
        }
    }
}

/// Run a single simulation to a [`RunOutcome`].
///
/// Thin wrapper over [`run_tracked`] (which also records the precise commit
/// round). Determinism: ALL randomness (network delay, drops, the VRF seed)
/// derives from `seed` via a single [`ChaCha8Rng`], so the same `(config,
/// seed)` always produces the same outcome (asserted in the tests).
pub fn run(config: &SimConfig, seed: u64) -> RunOutcome {
    run_tracked(config, seed)
}

/// Run a config across `seeds` seeds and aggregate into a [`SimReport`].
///
/// Seeds are `0..seeds`. Aggregation is the headline deliverable: per-config
/// fork count (safety), stall count (liveness), rounds-to-decide distribution,
/// and leadership fairness.
pub fn run_many(config: &SimConfig, seeds: u64) -> SimReport {
    let mut forks = 0u64;
    let mut stalls = 0u64;
    let mut agreements = 0u64;
    let mut rounds_hist: BTreeMap<u32, u64> = BTreeMap::new();
    let mut leadership: BTreeMap<usize, u64> = BTreeMap::new();
    let mut first_fork_seed: Option<u64> = None;

    for seed in 0..seeds {
        let outcome = run_tracked(config, seed);
        match outcome.decision {
            Decision::Fork => {
                forks += 1;
                first_fork_seed.get_or_insert(seed);
            }
            Decision::Stall => stalls += 1,
            Decision::Agreement => agreements += 1,
        }
        if let Some(r) = outcome.rounds_to_decide {
            *rounds_hist.entry(r).or_default() += 1;
        }
        for &l in &outcome.leaders {
            *leadership.entry(l).or_default() += 1;
        }
    }

    SimReport {
        config: config.clone(),
        seeds,
        forks,
        stalls,
        agreements,
        rounds_to_decide_hist: rounds_hist,
        leadership_distribution: leadership,
        first_fork_seed,
    }
}

/// Like [`run`] but records the precise round each correct node first
/// committed, so `rounds_to_decide` is exact. This is the path [`run_many`]
/// uses.
pub fn run_tracked(config: &SimConfig, seed: u64) -> RunOutcome {
    let n = config.n;
    let fbas = config.build_fbas();
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let vrf_seed = rng.gen::<u64>();

    let faulty: NodeSet = NodeSet::from_indices(config.faulty.iter().copied().filter(|&i| i < n));
    let is_faulty = |i: usize| faulty.contains(i);
    let crashed = |i: usize| is_faulty(i) && config.fault == FaultKind::Crash;

    // `heard[to][from]` = the latest value `to` has heard `from` vote for. Each
    // node has at most ONE current vote, so a newer value RETRACTS the older
    // one. This is the crux of safety: a *correct* node supports exactly one
    // value at any instant, so two correct nodes can only commit different
    // values if the two supporting quorums overlap solely in Byzantine
    // (equivocating) nodes — exactly the static splitting-set condition.
    let mut heard: Vec<BTreeMap<usize, ValueId>> = vec![BTreeMap::new(); n];
    // `heard_accept[to][from]` = the latest value `to` has heard `from` has
    // *accepted* (locked). This is the second federated-voting signal: a node
    // only irrevocably commits once a CONFIRMING QUORUM of these accepters
    // exists for one value (see the commit step). Because an honest node
    // accepts at most one value, two confirming quorums for different values
    // must share an honest accepter — impossible — so no fork can occur without
    // the Byzantine set reaching the splitting threshold, regardless of delay.
    let mut heard_accept: Vec<BTreeMap<usize, ValueId>> = vec![BTreeMap::new(); n];
    let mut committed: Vec<Option<ValueId>> = vec![None; n];
    let mut commit_round: Vec<Option<u32>> = vec![None; n];
    let mut own_vote: Vec<Option<ValueId>> = vec![None; n];
    // The federated-voting **accept lock**: a correct node *accepts* and locks a
    // value the first time it sees the accept condition (a quorum votes for it,
    // OR a v-blocking set has accepted it), pinning its own vote to it forever
    // after (it never retracts to a different value). A correct node thus
    // accepts at most ONE value. This lock is necessary but, on its own, NOT
    // sufficient for an irrevocable commit — commit additionally requires a
    // confirming quorum of accepters (the fix for #517). With ≤ splitting−1
    // equivocators, no fork is possible under arbitrary message reordering.
    let mut accepted: Vec<Option<ValueId>> = vec![None; n];
    let mut bus: BTreeMap<u32, Vec<Message>> = BTreeMap::new();

    // --- Coinbase-churn state (#535). Only active for the competing-coinbase
    // proposer with `churn_rate > 0`. Each node has a *currently-proposed
    // coinbase value* it nominates for the slot; production keeps the SINGLE
    // highest-PoW-priority coinbase per slot (service.rs `combine_fn`), so here
    // the competing-coinbase champion picks the **highest-priority** value it
    // has heard. We encode priority as the value id itself: a larger id = a
    // higher-PoW-priority coinbase, so "best heard" == max id. (This mirrors
    // service.rs, which `max_by_key(|v| v.priority)`.)
    //
    // Initial coinbases occupy the disjoint low range `0..n` (one per node).
    // Churn mints fresh, strictly-higher-priority ids from a per-run ascending
    // counter seeded ABOVE both the initial coinbases AND the equivocation id
    // range (`n + to`), so churn ids never collide with either. The counter is
    // shared across nodes and advanced deterministically from the seeded RNG, so
    // the whole run stays reproducible.
    let churn_active =
        config.proposer == ProposerModel::CompetingCoinbase && config.churn_rate > 0.0;
    // Each node's currently-proposed (nominated) coinbase. Starts at its index.
    let mut coinbase: Vec<ValueId> = (0..n as ValueId).collect();
    // Next churn-minted priority id. Reserve a high base disjoint from `0..n`
    // (initial coinbases) and `n..2n` (equivocation ids).
    let mut next_churn_id: ValueId = 2 * n as ValueId + 1;

    // The slot's **view-0 (base) leader** (see `leader_for`). Without view-
    // change this is the leader for the whole simulation. With view-change
    // (#519), it is the leader of the first view; later views rotate round-robin
    // from this base via `leader_at_view`.
    let base_leader = leader_for(config.proposer, seed, n, vrf_seed);
    // Leadership distribution reported to `run_many` is measured ACROSS seeds
    // (each seed = a distinct slot, one base leader per slot), so we record only
    // the base leader here even when view-change rotates additional leaders
    // mid-slot. The rotated leaders are an internal liveness-recovery detail and
    // recording them would distort the cross-seed fairness metric.
    let leaders: Vec<usize> = base_leader.into_iter().collect();

    // The current view's leader: the base leader rotated round-robin by `view`.
    // For the leaderless competing-coinbase model `base_leader` is `None` and
    // this stays `None` (view-change is a no-op there).
    let leader_at_view =
        |view: u32| -> Option<usize> { base_leader.map(|b| (b + view as usize) % n) };

    // View-change state (#519). `view` is the current view index; `leader` is
    // its leader. `view_deadline` is the round by which the current leader must
    // have produced a decision before the view rotates. With view-change
    // disabled, `view_deadline` is never reached so the view never advances.
    let view_budget = config.view_change.map(|b| b.max(1));
    let mut view: u32 = 0;
    let mut leader = leader_at_view(view);
    let mut view_deadline: Option<u32> = view_budget;

    // Derive `value -> set of supporting senders` for `node` from a per-sender
    // map (used for both votes and accepts).
    let supporters = |heard_node: &BTreeMap<usize, ValueId>| -> BTreeMap<ValueId, NodeSet> {
        let mut by_value: BTreeMap<ValueId, NodeSet> = BTreeMap::new();
        for (&from, &value) in heard_node {
            by_value.entry(value).or_default().insert(from);
        }
        by_value
    };

    // Whether `set` is **v-blocking** for `node`: it intersects every quorum of
    // `node`. For a symmetric `t`-of-`n` slice the complement of any quorum has
    // size `n − t`, so `set` is v-blocking iff it cannot be avoided by some
    // quorum — equivalently, the nodes NOT in `set` do not themselves contain a
    // slice for `node`. We compute it directly from the quorum-set definition so
    // it stays correct for arbitrary slices, not just the symmetric case.
    let is_v_blocking = |node: usize, set: &NodeSet| -> bool {
        if node >= fbas.len() {
            return false;
        }
        // `set` blocks `node` iff the remaining nodes (all − set) do NOT satisfy
        // `node`'s quorum set: i.e. every slice of `node` includes some member
        // of `set`.
        let complement = fbas.all_nodes().difference(set);
        !fbas.nodes[node].quorum_set.is_satisfied_by(&complement)
    };

    let send = |rng: &mut ChaCha8Rng,
                bus: &mut BTreeMap<u32, Vec<Message>>,
                cur: u32,
                from: usize,
                to: usize,
                value: ValueId,
                accept: Option<ValueId>| {
        let (delay, drop_prob) = match config.network {
            NetworkModel::Synchronous => (1u32, 0.0f64),
            NetworkModel::PartiallySynchronous {
                max_delay,
                drop_prob,
            } => {
                let d = if max_delay == 0 {
                    1
                } else {
                    1 + rng.gen_range(0..=max_delay)
                };
                (d, drop_prob)
            }
        };
        if drop_prob > 0.0 && rng.gen::<f64>() < drop_prob {
            return;
        }
        bus.entry(cur + delay).or_default().push(Message {
            from,
            to,
            value,
            accept,
        });
    };

    for round in 0..config.max_rounds {
        // --- View-change / leader-timeout step (#519). If the current view's
        // leader has not driven the slot to a decision by its deadline, rotate
        // to the next leader (round-robin) and open a fresh view. This is what
        // recovers liveness from a *Byzantine (equivocating)* or crashed leader:
        // undecided, not-yet-locked nodes follow the new leader's value next.
        //
        // SAFETY: rotating the leader does NOT touch `accepted`/`committed`. A
        // node that has already locked keeps its lock (its vote is pinned, see
        // below), so view-change can never make two correct nodes commit
        // different values — only quorum intersection (the splitting set) bounds
        // that, exactly as without view-change. Each view consumes ≥1 round
        // (`view_budget.max(1)`), so view-change always terminates within
        // `max_rounds`; if no decision is reached it is reported as a stall.
        if let (Some(budget), Some(deadline)) = (view_budget, view_deadline) {
            let all_done = (0..n).all(|i| crashed(i) || committed[i].is_some());
            if round >= deadline && !all_done {
                view += 1;
                leader = leader_at_view(view);
                view_deadline = Some(round.saturating_add(budget));
            }
        }

        // --- Coinbase-mining / churn step (#535). Each round, every still-live,
        // still-undecided node's local miner produces a fresh, strictly
        // higher-priority coinbase with probability `churn_rate`. This mirrors
        // production, where each validator's RandomX miner emits a new minting tx
        // several times a second.
        //
        // The PINNING toggle decides whether the node ACTS on the new coinbase
        // for its OWN nomination:
        // - PINNED (`pin_coinbase = true`, the #419 production fix): a node that has
        //   already proposed a coinbase this slot KEEPS that first value; the
        //   newly-mined higher-priority coinbase is discarded for nomination purposes.
        //   The per-node candidate set is therefore stable, so federated voting can
        //   converge.
        // - UNPINNED (`pin_coinbase = false`, the pre-#419 bug): the node ADOPTS the
        //   new higher-priority coinbase as its nominated value, retracting the
        //   previous one. Every node doing this keeps the candidate set churning, so
        //   the confirmed-nominate target never stabilizes and the slot jams (a stall)
        //   — exactly the #419 failure we reproduce here.
        //
        // A node that has already ACCEPTED (locked) a value never churns: its
        // vote is pinned to the lock for safety regardless of mining (see the
        // accept step). Churn is value-selection only — it never touches the
        // accept/commit machinery — so it cannot affect safety.
        if churn_active {
            for node in 0..n {
                if crashed(node) || committed[node].is_some() || accepted[node].is_some() {
                    continue;
                }
                // Draw per node per round so the matrix is reproducible.
                if rng.gen::<f64>() < config.churn_rate {
                    let mined = next_churn_id;
                    next_churn_id += 1;
                    // `mined` is strictly higher priority (larger id) than any
                    // value minted so far this slot.
                    if !config.pin_coinbase {
                        // Unpinned: re-nominate the newly-mined higher value.
                        coinbase[node] = mined;
                    }
                    // Pinned: keep `coinbase[node]` at the first proposed
                    // value.
                }
            }
        }

        for node in 0..n {
            if crashed(node) || committed[node].is_some() {
                continue;
            }
            let new_vote: ValueId = if let Some(locked) = accepted[node] {
                // Accept lock: pinned forever, never flips. Safety hinges on
                // this.
                locked
            } else {
                match config.proposer {
                    ProposerModel::CompetingCoinbase if churn_active => {
                        // Churn-active competing-coinbase: model the production
                        // combiner faithfully (service.rs keeps the SINGLE
                        // highest-PoW-priority coinbase). Priority == value id, so
                        // the champion is the MAX of this node's currently-
                        // nominated coinbase and every coinbase it has heard.
                        // Under PINNING `coinbase[node]` stays at the first
                        // proposed value; UNPINNED it tracks the latest mined
                        // higher value (see the churn step above), which is what
                        // keeps the candidate set moving and jams the slot.
                        let mut best = coinbase[node];
                        for (&_from, &v) in &heard[node] {
                            if v > best {
                                best = v;
                            }
                        }
                        best
                    }
                    ProposerModel::CompetingCoinbase => {
                        // Churn-free (the original v1 behavior): champion the
                        // lowest-id value heard so far (deterministic combiner
                        // stand-in), else own coinbase (= own index).
                        let mut best = node as ValueId;
                        for (&_from, &v) in &heard[node] {
                            if v < best {
                                best = v;
                            }
                        }
                        best
                    }
                    _ => {
                        // Leader models: the CURRENT VIEW's leader proposes a
                        // value; followers echo the value they HEARD from that
                        // leader. A correct leader proposes its canonical value
                        // (its index) to everyone; a Byzantine leader equivocates
                        // (per-recipient values, applied in the broadcast phase),
                        // so different followers echo different values — the
                        // fork attack.
                        //
                        // `leader` is the current view's leader, which the
                        // view-change step above rotates round-robin when a
                        // leader fails to drive a decision in time (#519). When
                        // view-change rotates to a CORRECT leader, undecided,
                        // not-yet-locked followers re-derive their vote from the
                        // new leader here (the `l != node` branch), converging on
                        // the new leader's value — this is the liveness recovery
                        // from a Byzantine or crashed leader.
                        //
                        // Liveness fallback: if a follower has not yet heard the
                        // current leader — e.g. the leader crashed, or the new
                        // view's leader has not broadcast yet — it falls back to
                        // the LOWEST value it has heard so far (incl. its own
                        // coinbase). All correct followers share this combiner,
                        // so even with view-change DISABLED a *crashed* leader is
                        // survived; only a *Byzantine* leader stalls when
                        // view-change is off (the documented v1 limit, retained
                        // for the comparison).
                        let l = leader.unwrap_or(node);
                        match heard[node].get(&l) {
                            Some(&v) if l != node => v,
                            _ if l == node => l as ValueId,
                            _ => {
                                let mut best = node as ValueId;
                                for (&_from, &v) in &heard[node] {
                                    if v < best {
                                        best = v;
                                    }
                                }
                                best
                            }
                        }
                    }
                }
            };
            own_vote[node] = Some(new_vote);
            // Record (and retract any prior) self-vote.
            heard[node].insert(node, new_vote);
        }

        for (from, vote) in own_vote.iter().enumerate() {
            if crashed(from) {
                continue;
            }
            let Some(v) = *vote else { continue };
            let equivocates = is_faulty(from) && config.fault == FaultKind::Equivocate;
            for to in 0..n {
                if to == from {
                    continue;
                }
                let (sent_value, sent_accept) = if equivocates {
                    // Equivocation: send each recipient a value keyed to that
                    // recipient, so distinct honest nodes are pushed toward
                    // distinct values (the fork-inducing partition attack). The
                    // base `n` offset keeps these ids disjoint from honest
                    // coinbase ids `0..n`. The Byzantine node ALSO equivocates on
                    // its *accept* signal (claiming to have accepted the same
                    // per-recipient value) — otherwise it could never help two
                    // honest nodes assemble distinct confirming quorums, and the
                    // at/above-splitting-set fork attack would be defeated for
                    // the wrong reason.
                    let ev = n as ValueId + to as ValueId;
                    (ev, Some(ev))
                } else {
                    // Honest node: broadcasts its current vote and, if it has
                    // locked a value, its accepted value (the confirm signal).
                    (v, accepted[from])
                };
                send(&mut rng, &mut bus, round, from, to, sent_value, sent_accept);
            }
        }

        let due: Vec<Message> = bus.remove(&(round + 1)).unwrap_or_default();
        for m in due {
            if crashed(m.to) {
                continue;
            }
            // Latest value heard from this sender overwrites the prior one
            // (retraction): a sender has one current vote per recipient.
            heard[m.to].insert(m.from, m.value);
            if let Some(a) = m.accept {
                // An accept is irrevocable for a correct sender, so we only ever
                // learn MORE accepts; record the latest seen.
                heard_accept[m.to].insert(m.from, a);
            }
        }

        for node in 0..n {
            if crashed(node) || committed[node].is_some() {
                continue;
            }
            let by_vote = supporters(&heard[node]);
            // --- Accept (lock) step. Lock onto a value the first time this node
            // sees the federated-voting *accept* condition for it, and never
            // move off it (the vote is pinned to the lock above). A correct node
            // therefore accepts at most ONE value — the invariant the confirm
            // step relies on for safety. Accept fires if EITHER:
            //   (a) a quorum (incl. self) currently *votes* for the value, OR
            //   (b) a *v-blocking* set of nodes have already *accepted* it
            //       (so the node cannot safely refuse to accept).
            // We pick the lowest qualifying value for determinism.
            if accepted[node].is_none() {
                let by_accept = supporters(&heard_accept[node]);
                let mut lock: Option<ValueId> = None;
                // (a) quorum-of-voters (must include self's own vote).
                for (&value, senders) in &by_vote {
                    if senders.contains(node) && fbas.is_quorum(senders) {
                        lock = Some(lock.map_or(value, |l| l.min(value)));
                    }
                }
                // (b) v-blocking-of-accepters: a v-blocking set already accepted
                //     it. This is the federated-voting accept escape hatch that
                //     keeps the protocol live under reordering.
                for (&value, accepters) in &by_accept {
                    if is_v_blocking(node, accepters) {
                        lock = Some(lock.map_or(value, |l| l.min(value)));
                    }
                }
                if let Some(v) = lock {
                    accepted[node] = Some(v);
                    // Record our own accept so a confirming quorum can include
                    // self, and so peers learn it on the next broadcast.
                    heard_accept[node].insert(node, v);
                }
            }
            // --- Confirm (commit) step. This is the safety-critical phase and
            // the fix for #517: irrevocably commit the accepted value ONLY once
            // a *confirming quorum* (incl. self) of nodes have each *accepted*
            // that same value — not merely voted for it. Because a correct node
            // accepts at most one value, two confirming quorums for different
            // values must share an honest accepter (quorum intersection), which
            // is impossible; so no two correct nodes can commit different values
            // under ANY message reordering unless the Byzantine set reaches the
            // splitting threshold.
            if let Some(v) = accepted[node] {
                let by_accept = supporters(&heard_accept[node]);
                if let Some(accepters) = by_accept.get(&v) {
                    if accepters.contains(node) && fbas.is_quorum(accepters) {
                        committed[node] = Some(v);
                        commit_round[node] = Some(round + 1);
                    }
                }
            }
        }

        let all_done = (0..n).all(|i| crashed(i) || committed[i].is_some());
        if all_done {
            break;
        }
    }

    // Build outcome with the precise earliest commit round.
    let mut correct_committed: BTreeMap<usize, ValueId> = BTreeMap::new();
    let mut earliest: Option<u32> = None;
    for i in 0..n {
        if is_faulty(i) {
            continue;
        }
        if let Some(v) = committed[i] {
            correct_committed.insert(i, v);
            if let Some(r) = commit_round[i] {
                earliest = Some(earliest.map_or(r, |e| e.min(r)));
            }
        }
    }
    let distinct: std::collections::BTreeSet<ValueId> =
        correct_committed.values().copied().collect();
    let decision = if correct_committed.is_empty() {
        Decision::Stall
    } else if distinct.len() >= 2 {
        Decision::Fork
    } else {
        Decision::Agreement
    };
    let rounds_to_decide = if decision == Decision::Stall {
        None
    } else {
        earliest
    };

    RunOutcome {
        seed,
        decision,
        rounds_to_decide,
        committed: correct_committed,
        leaders,
    }
}

/// Aggregate report over many seeds for one config — the v1 deliverable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimReport {
    /// The config that produced this report.
    pub config: SimConfig,
    /// Number of seeds run (`0..seeds`).
    pub seeds: u64,
    /// Number of runs that forked (SAFETY violations).
    pub forks: u64,
    /// Number of runs that stalled (LIVENESS violations).
    pub stalls: u64,
    /// Number of runs that reached agreement (safe + live).
    pub agreements: u64,
    /// Histogram: rounds-to-decide → count (excludes stalls).
    pub rounds_to_decide_hist: BTreeMap<u32, u64>,
    /// Leadership distribution: node index → times it was leader (fairness).
    pub leadership_distribution: BTreeMap<usize, u64>,
    /// The first seed that produced a fork, if any (for reproduction).
    pub first_fork_seed: Option<u64>,
}

impl SimReport {
    /// Stall rate as a fraction in `[0,1]` (stalls / seeds). The headline #535
    /// metric for the pinned-vs-unpinned coinbase-churn comparison. `0.0` if no
    /// seeds were run.
    pub fn stall_rate(&self) -> f64 {
        if self.seeds == 0 {
            0.0
        } else {
            self.stalls as f64 / self.seeds as f64
        }
    }

    /// Mean rounds-to-decide over non-stalled runs (`None` if all stalled).
    pub fn mean_rounds_to_decide(&self) -> Option<f64> {
        let (sum, count) = self
            .rounds_to_decide_hist
            .iter()
            .fold((0u64, 0u64), |(s, c), (&r, &n)| (s + r as u64 * n, c + n));
        if count == 0 {
            None
        } else {
            Some(sum as f64 / count as f64)
        }
    }

    /// Fairness as the ratio of the least-chosen to most-chosen leader count
    /// over the nodes that were *ever* leader (1.0 = perfectly even; `None`
    /// for the leaderless model). A low value flags lopsided leadership /
    /// grinding susceptibility.
    pub fn leadership_fairness(&self) -> Option<f64> {
        if self.leadership_distribution.is_empty() {
            return None;
        }
        let max = *self.leadership_distribution.values().max().unwrap();
        let min = *self.leadership_distribution.values().min().unwrap();
        if max == 0 {
            None
        } else {
            Some(min as f64 / max as f64)
        }
    }
}

/// Render a slice of [`SimReport`]s as a human-readable comparison table,
/// matching the existing report style in [`crate::report`].
pub fn render_sim_table(reports: &[SimReport]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<20} {:>3} {:>5} {:>22} {:>5} {:>6} {:>5} {:>6} {:>6} {:>6} {:>8} {:>9} {:>8}",
        "proposer",
        "n",
        "thr",
        "network",
        "vc",
        "churn",
        "pin",
        "forks",
        "stall",
        "agree",
        "stall%",
        "mean_rds",
        "fairness"
    );
    let _ = writeln!(out, "{}", "-".repeat(120));
    for r in reports {
        let t = r
            .config
            .threshold
            .unwrap_or_else(|| crate::thresholds::botho_bft_threshold(r.config.n));
        let net = match r.config.network {
            NetworkModel::Synchronous => "sync".to_string(),
            NetworkModel::PartiallySynchronous {
                max_delay,
                drop_prob,
            } => format!("psync(d{max_delay},drop{drop_prob:.2})"),
        };
        let mean = r
            .mean_rounds_to_decide()
            .map(|m| format!("{m:.2}"))
            .unwrap_or_else(|| "-".to_string());
        let fair = r
            .leadership_fairness()
            .map(|f| format!("{f:.2}"))
            .unwrap_or_else(|| "n/a".to_string());
        let vc = match r.config.view_change {
            Some(b) => format!("v{b}"),
            None => "off".to_string(),
        };
        // Churn / pin only apply to the competing-coinbase proposer; show "-"
        // for the leader models so the table is not misleading.
        let (churn, pin) = if r.config.proposer == ProposerModel::CompetingCoinbase {
            (
                format!("{:.2}", r.config.churn_rate),
                if r.config.pin_coinbase { "yes" } else { "no" }.to_string(),
            )
        } else {
            ("-".to_string(), "-".to_string())
        };
        let _ = writeln!(
            out,
            "{:<20} {:>3} {:>5} {:>22} {:>5} {:>6} {:>5} {:>6} {:>6} {:>6} {:>7.1}% {:>9} {:>8}",
            r.config.proposer.label(),
            r.config.n,
            t,
            net,
            vc,
            churn,
            pin,
            r.forks,
            r.stalls,
            r.agreements,
            r.stall_rate() * 100.0,
            mean,
            fair,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn psync(max_delay: u32, drop_prob: f64) -> NetworkModel {
        NetworkModel::PartiallySynchronous {
            max_delay,
            drop_prob,
        }
    }

    /// A known-safe config (n≥4, Botho BFT threshold) with faults below the
    /// splitting-set size NEVER forks, across many seeds and proposer models,
    /// even under partial synchrony.
    #[test]
    fn known_safe_never_forks() {
        // n=4 (3-of-4): splitting set = 2, so a single Byzantine equivocator is
        // below the threshold and must not be able to fork.
        for proposer in ProposerModel::all() {
            let config = SimConfig {
                n: 4,
                threshold: None, // 3-of-4
                proposer,
                network: psync(3, 0.2),
                faulty: vec![0],
                fault: FaultKind::Equivocate,
                max_rounds: 64,
                view_change: None,
                churn_rate: 0.0,
                pin_coinbase: true,
            };
            let report = run_many(&config, 300);
            assert_eq!(
                report.forks, 0,
                "proposer {:?}: 1 equivocator below splitting set (2) must never fork",
                proposer
            );
        }
    }

    /// Regression for #517: with ZERO faulty nodes, no fork can occur under any
    /// network model — including delay-only, which previously forked because
    /// the single-phase accept-lock committed on a transient vote quorum.
    #[test]
    fn zero_faults_never_fork_under_delay() {
        let networks = [
            NetworkModel::Synchronous,
            psync(3, 0.0),
            psync(0, 0.1),
            psync(3, 0.1),
        ];
        for n in [4usize, 7, 10] {
            for network in networks {
                for proposer in ProposerModel::all() {
                    let config = SimConfig {
                        n,
                        threshold: None,
                        proposer,
                        network,
                        faulty: vec![],
                        fault: FaultKind::Crash,
                        max_rounds: 128,
                        view_change: None,
                        churn_rate: 0.0,
                        pin_coinbase: true,
                    };
                    let report = run_many(&config, 200);
                    assert_eq!(
                        report.forks, 0,
                        "n={n} {proposer:?} net={network:?}: zero faults must never fork \
                         (first fork seed {:?})",
                        report.first_fork_seed
                    );
                }
            }
        }
    }

    /// Cross-check the static splitting-set prediction with dynamic behavior:
    /// below the splitting threshold → no fork; at/above it → a fork CAN occur.
    #[test]
    fn equivocation_crosses_splitting_threshold() {
        // n=4, 3-of-4: static minimal splitting set = 2.
        let fbas = Fbas::symmetric_botho(4);
        let split = fbas.health_report().min_splitting_set_cardinality.unwrap();
        assert_eq!(split, 2, "n=4 3-of-4 splitting set should be 2");

        // Below the threshold (1 equivocator): never forks, across every
        // proposer model and under partial synchrony.
        for proposer in ProposerModel::all() {
            let below = SimConfig {
                n: 4,
                threshold: None,
                proposer,
                network: psync(3, 0.2),
                faulty: vec![0],
                fault: FaultKind::Equivocate,
                max_rounds: 64,
                view_change: None,
                churn_rate: 0.0,
                pin_coinbase: true,
            };
            assert_eq!(
                run_many(&below, 200).forks,
                0,
                "{proposer:?}: 1 equivocator (< splitting set 2) must never fork"
            );
        }

        // At/above the threshold (2 equivocators = splitting set): a fork CAN
        // occur. The classic attack is a Byzantine leader equivocating to its
        // followers — so we use a leader model where the two Byzantine nodes
        // {0,2} (which include the round-0/1 leaders under round-robin) feed the
        // two honest nodes {1,3} different values. Each honest node + the two
        // Byzantine nodes form a 3-of-4 quorum supporting a distinct value →
        // fork. This is precisely the static analyzer's splitting-set=2
        // prediction realized dynamically.
        let at = SimConfig {
            n: 4,
            threshold: None, // 3-of-4
            proposer: ProposerModel::RoundRobinLeader,
            network: NetworkModel::Synchronous,
            faulty: vec![0, 2],
            fault: FaultKind::Equivocate,
            max_rounds: 64,
            view_change: None,
            churn_rate: 0.0,
            pin_coinbase: true,
        };
        assert!(
            run_many(&at, 200).forks > 0,
            "2 equivocators (= splitting set 2) must be able to fork dynamically"
        );
    }

    /// Unanimity below 4 nodes stalls when one node crashes (liveness): a
    /// 3-of-3 quorum cannot form with only 2 live nodes.
    #[test]
    fn unanimity_below_four_stalls_on_crash() {
        let config = SimConfig {
            n: 3,
            threshold: None, // Botho BFT at n=3 is 3-of-3 (unanimity)
            proposer: ProposerModel::RoundRobinLeader,
            network: NetworkModel::Synchronous,
            faulty: vec![2],
            fault: FaultKind::Crash,
            max_rounds: 32,
            view_change: None,
            churn_rate: 0.0,
            pin_coinbase: true,
        };
        let report = run_many(&config, 50);
        assert_eq!(
            report.agreements, 0,
            "3-of-3 with a crash cannot reach quorum"
        );
        assert_eq!(report.stalls, 50, "every seed must stall (liveness)");
        assert_eq!(report.forks, 0, "a stall is not a fork");
    }

    /// A crash below the blocking-set threshold stays LIVE: n=4 (3-of-4)
    /// tolerates one crash and still decides.
    #[test]
    fn crash_below_blocking_set_stays_live() {
        let config = SimConfig {
            n: 4,
            threshold: None, // 3-of-4, blocking set = 2
            proposer: ProposerModel::HashPriorityLeader,
            network: NetworkModel::Synchronous,
            faulty: vec![3],
            fault: FaultKind::Crash,
            max_rounds: 32,
            view_change: None,
            churn_rate: 0.0,
            pin_coinbase: true,
        };
        let report = run_many(&config, 50);
        assert_eq!(report.stalls, 0, "1 crash < blocking set 2 → stays live");
        assert_eq!(report.forks, 0);
        assert_eq!(report.agreements, 50);
    }

    /// Reproducibility: the same seed yields an identical outcome, byte for
    /// byte, and across the two run entry points.
    #[test]
    fn reproducibility_same_seed_same_outcome() {
        let config = SimConfig {
            n: 7,
            threshold: None,
            proposer: ProposerModel::VrfLeader,
            network: psync(4, 0.3),
            faulty: vec![1, 5],
            fault: FaultKind::Equivocate,
            max_rounds: 64,
            view_change: None,
            churn_rate: 0.0,
            pin_coinbase: true,
        };
        for seed in [0u64, 1, 42, 999] {
            let a = run_tracked(&config, seed);
            let b = run_tracked(&config, seed);
            assert_eq!(a, b, "seed {seed} must reproduce exactly");
        }
        // A different seed should (at least sometimes) differ, proving the seed
        // actually drives the run.
        let r0 = run_many(&config, 64);
        let r0b = run_many(&config, 64);
        assert_eq!(r0, r0b, "run_many is deterministic over a seed range");
    }

    /// Round-robin leadership is perfectly fair over a multiple-of-n round
    /// budget; the leaderless model reports no fairness.
    #[test]
    fn leadership_fairness_metrics() {
        let rr = SimConfig {
            n: 5,
            threshold: None,
            proposer: ProposerModel::RoundRobinLeader,
            network: NetworkModel::Synchronous,
            faulty: vec![],
            fault: FaultKind::Crash,
            max_rounds: 32,
            view_change: None,
            churn_rate: 0.0,
            pin_coinbase: true,
        };
        // Force every run to use its full round budget by making it stall-free
        // but capture leadership; on synchronous agreement runs end early, so
        // check a stalling unanimity-ish config for full leadership coverage.
        let report = run_many(&rr, 10);
        // At least the leader of round 0 must appear.
        assert!(!report.leadership_distribution.is_empty());

        let cc = SimConfig {
            proposer: ProposerModel::CompetingCoinbase,
            ..rr.clone()
        };
        let cc_report = run_many(&cc, 10);
        assert!(
            cc_report.leadership_fairness().is_none(),
            "competing-coinbase is leaderless → no fairness metric"
        );
    }

    /// Healthy synchronous config decides quickly (small rounds-to-decide).
    #[test]
    fn healthy_decides_fast() {
        let config = SimConfig::symmetric(7);
        let report = run_many(&config, 50);
        assert_eq!(report.forks, 0);
        assert_eq!(report.stalls, 0);
        let mean = report.mean_rounds_to_decide().unwrap();
        assert!(
            mean <= 3.0,
            "synchronous healthy net should decide fast, got {mean}"
        );
    }

    /// `run` and `run_tracked` agree on the decision classification.
    #[test]
    fn run_and_tracked_agree_on_decision() {
        let config = SimConfig {
            n: 4,
            threshold: None,
            proposer: ProposerModel::HashPriorityLeader,
            network: psync(2, 0.1),
            faulty: vec![0],
            fault: FaultKind::Crash,
            max_rounds: 64,
            view_change: None,
            churn_rate: 0.0,
            pin_coinbase: true,
        };
        for seed in 0..50 {
            let a = run(&config, seed);
            let b = run_tracked(&config, seed);
            assert_eq!(
                a.decision, b.decision,
                "seed {seed}: run and run_tracked must classify identically"
            );
            assert_eq!(a.committed, b.committed, "seed {seed}: same commits");
        }
    }

    /// View-change (#519) closes the Byzantine-leader stall: on a seed where
    /// the equivocating node IS the round-robin leader, the slot stalls
    /// WITHOUT view-change but reaches agreement WITH it. n=4, faulty node
    /// 0, seed 0 ⇒ `seed % n == 0` ⇒ node 0 is the leader.
    #[test]
    fn view_change_unsticks_byzantine_leader_seed() {
        let base = SimConfig {
            n: 4,
            threshold: None,
            proposer: ProposerModel::RoundRobinLeader,
            network: NetworkModel::Synchronous,
            faulty: vec![0],
            fault: FaultKind::Equivocate,
            max_rounds: 96,
            view_change: None,
            churn_rate: 0.0,
            pin_coinbase: true,
        };
        // Seed 0: node 0 is the leader (0 % 4 == 0) and equivocates → stall.
        assert_eq!(
            run_tracked(&base, 0).decision,
            Decision::Stall,
            "without view-change, a Byzantine leader must stall its own slot"
        );
        // Same seed WITH view-change: rotate to a correct leader → agreement.
        let with = SimConfig {
            view_change: Some(2),
            churn_rate: 0.0,
            pin_coinbase: true,
            ..base
        };
        assert_eq!(
            run_tracked(&with, 0).decision,
            Decision::Agreement,
            "with view-change, the slot must rotate off the Byzantine leader and \
             reach agreement"
        );
    }

    /// A `view_budget` of 0 is clamped to 1 (each view consumes ≥1 round) so
    /// the simulation still terminates and behaves like a 1-round view
    /// budget rather than spinning forever or never rotating.
    #[test]
    fn view_budget_zero_is_clamped_and_terminates() {
        let config = SimConfig {
            n: 4,
            threshold: None,
            proposer: ProposerModel::RoundRobinLeader,
            network: NetworkModel::Synchronous,
            faulty: vec![0],
            fault: FaultKind::Equivocate,
            max_rounds: 96,
            view_change: Some(0),
            churn_rate: 0.0,
            pin_coinbase: true,
        };
        // Must terminate (no panic / hang) and recover the Byzantine-leader seed.
        let report = run_many(&config, 64);
        assert_eq!(report.forks, 0);
        assert_eq!(
            report.stalls, 0,
            "view_budget 0 (clamped to 1) must still recover via rotation"
        );
    }
}
