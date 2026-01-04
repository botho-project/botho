# Architecture Decision Records

This folder contains Architecture Decision Records (ADRs) for the Botho project. ADRs document significant architectural decisions along with their context and consequences.

## What is an ADR?

An Architecture Decision Record captures a decision that has significant impact on the project's architecture. Each ADR describes:

- **Context**: The situation and forces at play
- **Decision**: The chosen approach
- **Consequences**: What results from the decision (positive, negative, neutral)
- **Alternatives**: Other options considered and why they were rejected

ADRs are immutable once accepted. If a decision changes, a new ADR supersedes the old one rather than modifying it.

## Decision Index

| ADR | Title | Status | Date |
|-----|-------|--------|------|
| [0001](0001-deprecate-lion-ring-signatures.md) | Deprecate LION Ring Signatures | Accepted | 2026-01-03 |

## ADR Statuses

| Status | Meaning |
|--------|---------|
| **Proposed** | Under discussion, not yet accepted |
| **Accepted** | Decision approved and in effect |
| **Deprecated** | Superseded by a later ADR |
| **Rejected** | Considered but not adopted |

## Creating a New ADR

### 1. Choose the Next Number

ADRs are numbered sequentially. Check the highest existing number and increment by 1.

### 2. Create the File

Create a new file: `docs/decisions/NNNN-short-title.md`

Example: `docs/decisions/0002-adopt-mlkem-stealth-addresses.md`

### 3. Use the Template

```markdown
# ADR NNNN: Title

**Status**: Proposed
**Date**: YYYY-MM-DD
**Decision Makers**: [Team/Individual]

## Context

[Describe the situation. What problem are you solving? What constraints exist?]

## Problem Statement

[Clear description of the specific issue being addressed]

## Decision

[State the decision clearly and concisely]

## Consequences

### Positive

1. [Benefit 1]
2. [Benefit 2]

### Negative

1. [Drawback 1]
2. [Drawback 2]

### Neutral

1. [Trade-off 1]
2. [Trade-off 2]

## Alternatives Considered

### 1. [Alternative Name]

- Pro: [Advantage]
- Con: [Disadvantage]

### 2. [Alternative Name]

- Pro: [Advantage]
- Con: [Disadvantage]

## Implementation

[Optional: Steps to implement the decision]

## References

- [Link to relevant documentation]
- [Link to related discussions]
```

### 4. Submit for Review

1. Create a branch for your ADR
2. Open a pull request
3. Tag relevant stakeholders for review
4. Update status to "Accepted" once approved

## When to Write an ADR

Write an ADR when making decisions that:

- **Affect architecture**: Changes to system structure, component boundaries, or data flow
- **Have lasting impact**: Choices that are costly to reverse later
- **Involve trade-offs**: Multiple valid options with different consequences
- **Need documentation**: Future team members will ask "why did we do this?"

Examples of ADR-worthy decisions:
- Choosing a cryptographic algorithm
- Selecting a consensus mechanism
- Deprecating a major feature
- Changing the network protocol
- Adopting a new storage format

## When NOT to Write an ADR

Skip the ADR for:
- Bug fixes
- Performance optimizations without architectural changes
- Library version updates
- Code style changes
- Documentation improvements

## References

- [Michael Nygard's ADR article](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
- [ADR GitHub organization](https://adr.github.io/)
