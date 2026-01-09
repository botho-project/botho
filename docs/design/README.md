# Design Documents

This section contains design proposals, roadmaps, and technical specifications for planned or in-progress features.

## Economic Design

### Core Mechanisms

| Document | Status | Description |
|----------|--------|-------------|
| [Minting Proximity Fees](minting-proximity-fees.md) | Research | Conceptual framing: cluster tags track minting origin |
| [Cluster Tag Decay](cluster-tag-decay.md) | Implemented | Phase 1: Age-based decay mechanism |
| [Entropy-Weighted Decay](entropy-weighted-decay.md) | Proposed | Phase 2: Commerce-sensitive decay |
| [Progressive Fees](../concepts/progressive-fees.md) | Implemented | Fee curve and economic effects |

### Redistribution Mechanisms

| Document | Status | Description |
|----------|--------|-------------|
| [Asymmetric UTXO Fees](asymmetric-utxo-fees.md) | Research | Combined mechanism: asymmetric fees + value-weighted lottery + eligibility decay |
| [Asymmetric Fees Simulation](asymmetric-fees-simulation.md) | Draft | Simulation specification for combined mechanism |
| [Lottery Redistribution](lottery-redistribution.md) | Reference | Background analysis of lottery trade-offs |

### Entropy Proofs

| Document | Status | Description |
|----------|--------|-------------|
| [Entropy Proof Integration](entropy-proof-integration.md) | Implemented | Bulletproofs integration for cluster entropy |
| [Entropy Proof Security Analysis](entropy-proof-security-analysis.md) | Complete | Formal security analysis |
| [Entropy Proof Aggregation Research](entropy-proof-aggregation-research.md) | Research | Aggregation techniques |

## Privacy

| Document | Status | Description |
|----------|--------|-------------|
| [Ring Signature Privacy Analysis](ring-signature-privacy-analysis.md) | Complete | Privacy guarantees analysis |
| [Ring Signature Tag Propagation](ring-signature-tag-propagation.md) | Complete | Tag behavior through rings |

## Network & Security

| Document | Status | Description |
|----------|--------|-------------|
| [PoW Connection Challenge](pow-connection-challenge.md) | Implemented | DDoS protection via proof-of-work |
| [Traffic Privacy Roadmap](traffic-privacy-roadmap.md) | Draft | Onion routing and protocol obfuscation |

## Archived

Withdrawn or superseded designs are moved to [archive/](archive/). Each archived document explains why it was withdrawn and what supersedes it.

| Document | Reason |
|----------|--------|
| [wealth-conditional-privacy.md](archive/wealth-conditional-privacy.md) | Based on flawed premise |
| [wealth-privacy-simulation.md](archive/wealth-privacy-simulation.md) | Parent design withdrawn |
| [provenance-based-selection.md](archive/provenance-based-selection.md) | Superseded by combined mechanism |

## Document Lifecycle

1. **Research** - Exploring feasibility, seeking feedback
2. **Proposed** - Detailed design, under review
3. **Draft** - Specification being written
4. **Accepted** - Approved for implementation
5. **Implemented** - In production
6. **Complete** - Analysis/research complete
7. **Reference** - Background material, still valid
8. **Archived** - Withdrawn or superseded (see [archive/](archive/))

## Contributing

To propose a new design:

1. Create a new markdown file in this directory
2. Use the template structure from existing documents
3. Open a PR with the `design` label
4. Request review from core contributors
