//! Inactive model adapter only. Never called by production validation.
//! Caller obtains available/cap/eligible from the unchanged production kernels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Baseline,
    ThresholdReserve,
    AdaptiveCount,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Decision {
    pub distribution: u64,
    pub winners: usize,
    pub reserve: u128,
    pub reason: &'static str,
}
pub const THRESHOLD: u64 = 250_001_000_000;

pub fn decide(policy: Policy, available: u128, cap: u64, eligible: usize) -> Decision {
    let budget = available.min(cap as u128) as u64;
    let slots = eligible.min(4);
    let (winners, reason) = if slots == 0 {
        (0, "no_eligible")
    } else if budget == 0 {
        (0, "zero_budget")
    } else {
        match policy {
            Policy::Baseline => (slots, "draw"),
            Policy::ThresholdReserve if budget / slots as u64 >= THRESHOLD => (slots, "draw"),
            Policy::ThresholdReserve => (
                0,
                if cap / (slots as u64) < THRESHOLD {
                    "cap_stall"
                } else {
                    "accumulating"
                },
            ),
            Policy::AdaptiveCount => {
                let n = (budget / THRESHOLD).min(slots as u64) as usize;
                (
                    n,
                    if n > 0 {
                        "draw"
                    } else if cap < THRESHOLD {
                        "cap_stall"
                    } else {
                        "accumulating"
                    },
                )
            }
        }
    };
    let distribution = if winners == 0 { 0 } else { budget };
    Decision {
        distribution,
        winners,
        reserve: available - distribution as u128,
        reason,
    }
}
