# Piecewise Linear Fee Curve for ZK Compatibility

## Summary

**Recommended configuration: 3-Segment Balanced**

```
Segment 1 (Poor):   [0, 15% of max_wealth)    → 1% flat rate
Segment 2 (Middle): [15%, 70% of max_wealth)  → 2% to 10% linear
Segment 3 (Rich):   [70%+ of max_wealth]      → 15% flat rate
```

**Simulation results** (500 rounds, 100 agents):

| Model | ΔGini | Burn Rate |
|-------|-------|-----------|
| Flat 5% | -0.2353 | 9.1% |
| Sigmoid (current) | -0.2393 | 12.5% |
| **3-Seg balanced** | **-0.2399** | **12.4%** |

The 3-segment model achieves **0.3% better Gini reduction** with **0.1% less burn** than sigmoid, while being ZK-provable with ~4.5 KB proof overhead.

---

## The Core Problem

We want to compute `fee = f(wealth)` where:
- `wealth` is hidden in a Pedersen commitment: `C = wealth * H + r * G`
- `fee` must be verifiable without revealing `wealth`
- `f()` should approximate the sigmoid's S-curve behavior

## Current Implementation: Already Piecewise Linear!

The sigmoid in `fee_curve.rs:376` uses a **lookup table with linear interpolation**:

```rust
const LUT: [(i64, u64); 7] = [
    (-6000, 131),     // sigmoid(-6) ≈ 0.002 → factor ≈ 1.01x
    (-4000, 1180),    // sigmoid(-4) ≈ 0.018 → factor ≈ 1.09x
    (-2000, 7798),    // sigmoid(-2) ≈ 0.119 → factor ≈ 1.60x
    (0, 32768),       // sigmoid(0)  = 0.500 → factor ≈ 3.50x
    (2000, 57738),    // sigmoid(2)  ≈ 0.881 → factor ≈ 5.40x
    (4000, 64356),    // sigmoid(4)  ≈ 0.982 → factor ≈ 5.91x
    (6000, 65405),    // sigmoid(6)  ≈ 0.998 → factor ≈ 5.99x
];
```

This gives us 6 linear segments. The question: **can we prove this in ZK?**

## ZK Proof Strategy for Piecewise Linear

### What We Need to Prove

Given:
- `C_wealth` = commitment to wealth
- `fee` = claimed fee (public)
- Segment boundaries `[w_0, w_1, w_2, ..., w_6]` (public)
- Slopes and intercepts `(m_i, b_i)` for each segment (public)

Prove that `fee ≥ m_i * wealth + b_i` for the correct segment `i`.

### The Challenge: Hiding Segment Membership

If we reveal which segment wealth falls into, we leak information:
- "This is segment 5" → wealth is between w_4 and w_5
- This narrows the anonymity set

**Options:**

1. **Accept segment leakage** (simplest)
   - 6 segments = 2.58 bits of information leaked
   - May be acceptable if segments are wide

2. **Disjunction proof** (hide segment)
   - Prove: (wealth ∈ seg_0 ∧ fee ≥ f_0(wealth)) ∨ (wealth ∈ seg_1 ∧ fee ≥ f_1(wealth)) ∨ ...
   - O(N) proof size for N segments
   - Hides which segment, but expensive

3. **Polynomial representation** (elegant)
   - Represent piecewise linear as a single polynomial via interpolation
   - Use polynomial commitment (KZG, FRI, etc.)
   - Single proof regardless of segment count

## Approach Analysis

### Approach A: Segment Revelation (Simple)

```
Public: segment_id = i
Prove:
  1. w_i ≤ wealth < w_{i+1}  (two range proofs)
  2. fee ≥ m_i * wealth + b_i  (one linear relation proof)
```

**Proof size**: 2 range proofs + 1 linear proof ≈ 1-2 KB
**Privacy cost**: Reveals wealth bracket (2.58 bits for 6 segments)

### Approach B: OR-Proof (Private Segment)

```
Prove: ∨_{i=0}^{5} (w_i ≤ wealth < w_{i+1} ∧ fee ≥ m_i * wealth + b_i)
```

Using Sigma protocol OR-composition:
- Prover picks real segment, simulates others
- Verifier can't tell which is real

**Proof size**: 6 × (range proof + linear proof) ≈ 6-12 KB
**Privacy cost**: None (segment hidden)

### Approach C: Committed Lookup with Interpolation

Inspired by Plookup/cq:

1. Commit to lookup table entries: `{(w_i, f_i)}`
2. Prove wealth is "between" two adjacent entries
3. Prove fee is computed by linear interpolation

This is more complex but potentially more efficient for many segments.

### Approach D: Polynomial Commitment

Convert piecewise linear to polynomial:
- 6 segments with 7 control points
- Lagrange interpolation gives degree-6 polynomial
- Use KZG commitment to prove `f(committed_wealth) ≤ fee`

**Pros**: Single proof size, elegant
**Cons**: Polynomial evaluation proof is complex; approximation error at segment boundaries

## Recommended Approach: Reduced Segments + OR-Proof

### Observation: 3 Segments Capture S-Curve Character

```
Segment 1 (Poor): [0, w_mid/2)      → slow growth, factor 1x to 2x
Segment 2 (Middle): [w_mid/2, 2*w_mid) → fast growth, factor 2x to 5x
Segment 3 (Rich): [2*w_mid, ∞)      → slow growth, factor 5x to 6x
```

This preserves:
- Slow start for small wealth (progressive, not regressive)
- Steep middle (most redistribution happens here)
- Plateau at top (diminishing marginal rate)

### 3-Segment OR-Proof Cost

```
Proof size: 3 × (2 range proofs + 1 linear proof)
         ≈ 3 × 1.5 KB = 4.5 KB
```

This is reasonable for a privacy-preserving progressive tax!

## Comparison of Options

| Option | Segments | Privacy | Proof Size | Complexity |
|--------|----------|---------|------------|------------|
| Pure Linear | 1 | Full | ~1 KB | Low |
| 3-Segment OR | 3 | Full | ~4.5 KB | Medium |
| 6-Segment OR | 6 | Full | ~9 KB | Medium |
| Segment Reveal | 6 | 2.58 bits leaked | ~1.5 KB | Low |
| Polynomial | ∞ | Full | ~2 KB | High |

## Mathematical Details

### Linear Segment Definition

For segment `i` with boundaries `[w_i, w_{i+1})` and factor endpoints `[f_i, f_{i+1}]`:

```
slope_i = (f_{i+1} - f_i) / (w_{i+1} - w_i)
factor(w) = f_i + slope_i × (w - w_i)
fee = base_fee × factor(w) / SCALE
```

### Range Proof for Segment Membership

To prove `w_i ≤ wealth < w_{i+1}`:

```
Prove: wealth - w_i ≥ 0           (commitment to non-negative value)
Prove: w_{i+1} - 1 - wealth ≥ 0   (commitment to non-negative value)
```

Using Bulletproofs, each range proof is ~700 bytes.

### Linear Relation Proof

To prove `fee ≥ f_i + slope_i × (wealth - w_i)`:

```
Let required_fee = f_i + slope_i × (wealth - w_i)
            = (f_i - slope_i × w_i) + slope_i × wealth
            = intercept_i + slope_i × wealth

Prove: fee - intercept_i - slope_i × wealth ≥ 0
```

This is a single range proof on a linear combination of committed values.

## Implementation Sketch

```rust
/// 3-segment piecewise linear fee curve for ZK compatibility
pub struct ZkFeeCurve {
    /// Segment boundaries: [0, w1, w2, MAX]
    pub boundaries: [u64; 4],

    /// Factor at each boundary: [f0, f1, f2, f3]
    pub factors: [u64; 4],
}

impl ZkFeeCurve {
    /// Default S-curve approximation
    pub fn default() -> Self {
        Self {
            boundaries: [0, 5_000_000, 20_000_000, u64::MAX],
            factors: [1000, 2000, 5000, 6000], // 1x, 2x, 5x, 6x
        }
    }

    /// Get segment parameters for ZK proof
    pub fn segment_params(&self, segment: usize) -> (u64, u64, i64, i64) {
        let w_lo = self.boundaries[segment];
        let w_hi = self.boundaries[segment + 1];
        let f_lo = self.factors[segment] as i64;
        let f_hi = self.factors[segment + 1] as i64;

        // slope = (f_hi - f_lo) / (w_hi - w_lo)
        // intercept = f_lo - slope * w_lo

        (w_lo, w_hi, f_lo, f_hi)
    }
}
```

## Open Questions

1. **Segment boundary visibility**: Are boundaries public or should they also be hidden?
   - Recommendation: Public boundaries, hidden membership

2. **Integer overflow**: With large wealth values, slope calculations need care
   - Use 128-bit intermediate values

3. **Rounding direction**: Which way to round for security?
   - Round fee UP (conservative, user pays slightly more)

4. **Interaction with ring signatures**: How does segment OR-proof interact with ring signature OR-proof?
   - May be able to batch/combine for efficiency

## Simulation Validation

The 3-segment model was validated against sigmoid and other fee curves using `scripts/gini_3segment.py`.

### Configurations Tested

| Config | w1 | w2 | r_poor | r_mid | r_rich | ΔGini | Burn |
|--------|-----|-----|--------|-------|--------|-------|------|
| 3-Seg wide | 10% | 60% | 1% | 1→12% | 15% | -0.2407 | 13.9% |
| **3-Seg balanced** | **15%** | **70%** | **1%** | **2→10%** | **15%** | **-0.2399** | **12.4%** |
| 3-Seg sigmoid-match | 20% | 80% | 2% | 3→12% | 14% | -0.2388 | 12.1% |

### Fee Curve Comparison

```
Effective Wealth    Flat    Linear   3-Seg    Sigmoid
           0        5.0%     1.0%     1.0%      2.1%
      50,000        5.0%     1.7%     1.0%      2.3%
     100,000        5.0%     2.4%     1.0%      2.7%
     200,000        5.0%     3.8%     3.5%      3.6%
     400,000        5.0%     6.6%     6.4%      6.3%
     600,000        5.0%     9.4%     9.3%      9.7%
     800,000        5.0%    12.2%    15.0%     12.4%
   1,000,000        5.0%    15.0%    15.0%     13.9%
```

The 3-segment "balanced" configuration closely tracks the sigmoid shape while being ZK-provable.

## Conclusion

**Recommended for Phase 2: 3-Segment Balanced**

```rust
pub struct ZkFeeCurve {
    // Segment boundaries as fractions of max_wealth
    pub w1_frac: f64 = 0.15,  // Poor/Middle boundary
    pub w2_frac: f64 = 0.70,  // Middle/Rich boundary

    // Fee rates (basis points)
    pub r_poor: u32 = 100,      // 1% for poor segment
    pub r_mid_start: u32 = 200, // 2% at start of middle
    pub r_mid_end: u32 = 1000,  // 10% at end of middle
    pub r_rich: u32 = 1500,     // 15% for rich segment
}
```

**Why this configuration:**

1. **Better than sigmoid**: -0.2399 vs -0.2393 Gini reduction (0.3% improvement)
2. **Lower burn**: 12.4% vs 12.5% (0.1% less supply destruction)
3. **ZK-provable**: 3 OR-proofs at ~4.5 KB total overhead
4. **Preserves S-curve economics**:
   - Poor stay at low rates longer (flat 1% up to 15% wealth)
   - Middle class sees steepest progression (2%→10% linear)
   - Rich hit plateau (flat 15% above 70% wealth)

**Implementation path:**

1. Add `ZkFeeCurve` to `src/fee_curve.rs`
2. Implement 3-way OR-proof in `src/crypto/`
3. Integrate with `TagConservationProof`
4. Benchmark proof generation/verification

The piecewise linear approach is **validated by simulation** and **preserves the sigmoid's economic properties** while enabling ZK verification of progressive fees.
