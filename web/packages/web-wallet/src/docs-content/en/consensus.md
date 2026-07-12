## Stellar Consensus Protocol

Botho uses the **Stellar Consensus Protocol (SCP)** for distributed consensus. SCP is a federated Byzantine agreement protocol that provides fast finality, energy efficiency, and flexible trust—without sacrificing decentralization.

### Why Not Proof-of-Work Consensus?

Proof-of-work (PoW) consensus, as used in Bitcoin, has significant drawbacks:

- **Energy waste** - PoW deliberately consumes massive amounts of electricity as a security mechanism
- **Slow finality** - Bitcoin transactions aren't truly final for an hour or more
- **Centralization pressure** - Mining economies of scale push toward industrial operations
- **51% attacks** - If an attacker controls majority hashpower, they can rewrite history

**Botho still uses proof-of-work — but only for coin issuance, never for consensus.** Blocks are minted through CPU-egalitarian RandomX mining, which decides *who earns the block reward*. Whether that block is *accepted* is decided entirely by SCP. This decoupling means an attacker with majority hashpower can out-earn everyone else, but cannot rewrite history or censor transactions — and because hashpower buys no security, there is no arms-race pressure toward Bitcoin-scale energy consumption.

### Why Not Proof-of-Stake?

Proof-of-stake (PoS) improves on energy usage but introduces its own issues:

- **Nothing-at-stake** - Validators can cheaply vote on multiple chain forks
- **Wealth concentration** - The rich get richer through staking rewards
- **Long-range attacks** - Old keys can potentially rewrite history
- **Complexity** - PoS systems require intricate slashing and validator selection logic

### How SCP Works

SCP takes a fundamentally different approach based on **federated voting**:

**Quorum Slices:** Each node in the network defines its own "quorum slice"—a set of other nodes it trusts. A node will only accept a statement as final when its quorum slice agrees.

**Quorum Intersection:** The network is secure as long as all quorum slices share some nodes in common. This ensures that two conflicting statements cannot both achieve consensus.

**Federated Voting:** Consensus proceeds through a series of voting rounds:

1. **Nominate** - Nodes propose candidate values for the next block
2. **Prepare** - Nodes vote to prepare a specific value
3. **Commit** - Nodes vote to commit the prepared value
4. **Externalize** - Once committed, the value is final

**Key Insight:** Unlike PoW where you trust "the longest chain," in SCP you explicitly choose which nodes to trust. This makes the trust model transparent and auditable.

### Properties of SCP

**Decentralized Control:** No central authority determines consensus. Each node independently chooses its quorum slice based on its own assessment of trustworthiness.

**Low Latency:** Transactions reach finality in seconds (typically 3-5 seconds under normal conditions), compared to minutes or hours for PoW systems.

**Flexible Trust:** Participants can choose different quorum configurations based on their needs. Some may trust established institutions; others may trust a set of technical experts.

**Asymptotic Security:** As the network grows and quorum slices become more interconnected, the system becomes more resilient against Byzantine failures.

**Energy Efficiency:** SCP nodes only need to exchange messages and verify signatures—no computational puzzles, no energy waste.

### Safety vs. Liveness

SCP prioritizes **safety** over **liveness**:

- **Safety:** The network will never confirm conflicting transactions
- **Liveness:** The network should eventually make progress

If the quorum structure is disrupted (too many nodes go offline), SCP will halt rather than risk confirming conflicting transactions. This is the correct trade-off for a monetary system—it's better to pause than to have funds stolen.

### Quorum Configuration in Botho

The Botho network starts with a bootstrap quorum centered on the foundation's seed nodes. Over time, as more independent nodes join, the quorum structure will become increasingly decentralized.

Node operators can customize their quorum slice to trust:
- The foundation's seed nodes (default)
- Other known community nodes
- Nodes run by exchanges or businesses they trust
- Any combination of the above

The health of the network depends on sufficient quorum intersection. The Botho explorer shows real-time quorum topology to help operators make informed decisions.
