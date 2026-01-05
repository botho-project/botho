# Design Documents

This section contains design proposals, roadmaps, and technical specifications for planned or in-progress features.

## Active Proposals

| Document | Status | Description |
|----------|--------|-------------|
| [Traffic Privacy Roadmap](traffic-privacy-roadmap.md) | Draft | Traffic analysis resistance via Onion Gossip |

## Economic Design

| Document | Status | Description |
|----------|--------|-------------|
| [Cluster Tag Decay](cluster-tag-decay.md) | Implemented | Mathematical model for fee decay |
| [Progressive Fees](../concepts/progressive-fees.md) | Implemented | Fee curve and economic effects |
| [Wealth-Conditional Privacy](wealth-conditional-privacy.md) | Proposal | Source-wealth threshold for amount visibility |
| [Wealth Privacy Simulation](wealth-privacy-simulation.md) | Draft | Simulation plan for privacy validation |
| [Lottery Redistribution](lottery-redistribution.md) | Proposal | Alternative redistribution mechanisms |
| [Provenance-Based Selection](provenance-based-selection.md) | Proposal | Entropy-weighted output selection |

## Network & Security

| Document | Status | Description |
|----------|--------|-------------|
| [PoW Connection Challenge](pow-connection-challenge.md) | Implemented | DDoS protection via proof-of-work |
| [Traffic Privacy Roadmap](traffic-privacy-roadmap.md) | Draft | Onion routing and protocol obfuscation |

## Document Lifecycle

1. **Proposal** - Initial idea, seeking feedback
2. **Draft** - Detailed design, under review
3. **Accepted** - Approved for implementation
4. **Implemented** - In production
5. **Deprecated** - Superseded or abandoned

## Contributing

To propose a new design:

1. Create a new markdown file in this directory
2. Use the template structure from existing documents
3. Open a PR with the `design` label
4. Request review from core contributors
