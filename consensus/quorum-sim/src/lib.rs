//! Static quorum-health analyzer for Botho's curated FBAS federation.
//!
//! This crate is **Deliverable 2 of #510**, scoped to **Path A** (a curated
//! validator federation + thin clients; see #427). It provides a small,
//! Botho-grounded re-implementation of the static metrics computed by tools
//! such as [`fbas_analyzer`](https://github.com/wiberlin/fbas_analyzer) and
//! [`python-fbas`](https://github.com/nano-o/python-fbas), tied directly to
//! Botho's threshold rule (`effective_threshold = n − floor((n−1)/3)`, see
//! [`botho/src/config.rs`]).
//!
//! # Scope (v1)
//!
//! This is a **static** analyzer plus threshold/growth comparison. It does
//! **not** simulate SCP message rounds, Byzantine equivocation, or partial
//! synchrony — that dynamic message-level simulator is an explicit follow-up
//! (see the issue's "Deferred" section).
//!
//! # Model
//!
//! An [`Fbas`] is a set of nodes, each with a [`QuorumSet`] (a threshold over a
//! list of validator members). Botho's curated federation is small
//! (`N ≤ ~20`), so every analysis here brute-forces over node subsets using
//! bitsets ([`NodeSet`]). This is exponential in `N` but exact, and trivially
//! fast at the sizes Path A targets.
//!
//! # Key metrics
//!
//! - [`Fbas::is_quorum`] — quorum predicate.
//! - [`Fbas::has_quorum_intersection`] — do all quorums pairwise intersect? (A
//!   `false` here means a fork is possible.)
//! - [`Fbas::minimal_quorums`] — the smallest quorums (by set inclusion).
//! - [`Fbas::minimal_blocking_sets`] — smallest sets whose failure halts the
//!   network (the **liveness** buffer).
//! - [`Fbas::minimal_splitting_sets`] — smallest sets whose Byzantine
//!   misbehaviour can fork the network (the **safety** buffer).

pub mod analysis;
pub mod model;
pub mod nodeset;
pub mod report;
pub mod thresholds;

pub use analysis::{HealthReport, ThresholdRule};
pub use model::{Fbas, Node, QuorumSet};
pub use nodeset::NodeSet;
