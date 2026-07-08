/**
 * Operator trust/quorum view types (#706, P4.1 of the #695 proposal).
 *
 * Public read surface only: every field maps 1:1 to a public RPC response —
 * the quorum promotion-gate fields of `node_getStatus` (#651/#509) and the
 * connected-peer table of `network_getPeers` (#544). No auth, no writes.
 *
 * Anti-#541 contract (mirrors `../network/types`):
 * - `reachable: false` means the poll failed — the card renders an explicit
 *   error state, never stale or fabricated values.
 * - Fields the node reports as JSON `null` (no gate evaluation yet — relay
 *   node or pre-first-rebuild) are `undefined` here and render as absent
 *   ("—"), never zero-filled.
 */

/** One connected-peer row from `network_getPeers` (`PeerInfoSnapshot`, #544). */
export interface TrustPeer {
  /** libp2p peer ID string. */
  peerId: string
  /** Last known multiaddr; null when none has been observed. */
  address: string | null
  /** Advertised protocol version; null when the peer is not yet identified. */
  protocolVersion: string | null
  /** True when the peer's protocol version is below the node's minimum. */
  versionWarning: boolean
  /** Seconds since the peer was last seen, at snapshot time. */
  lastSeenSecs?: number
}

/**
 * Per-node trust posture: `node_getStatus` gate fields merged with the
 * `network_getPeers` peer table for one ingress node.
 */
export interface NodeTrustStatus {
  nodeId: string
  /** False when the `node_getStatus` poll failed (timeout / network / RPC error). */
  reachable: boolean
  /** Unix millis when this snapshot was taken. */
  polledAt: number
  /** BFT posture (#509): true only with >= 4 participating nodes in recommended mode. */
  quorumFaultTolerant?: boolean
  /** The n-of-n / zero-fault-tolerance regime (#509). Warn, don't refuse. */
  quorumDegenerate?: boolean
  /** Curated (`[network.quorum] members`) quorum members. */
  quorumCuratedMembers?: number
  /** Auto-promoted quorum members (deterministic selection under the cap). */
  quorumAutoMembers?: number
  /** Discovered peers the gate is keeping OUT of the safety-critical quorum. */
  quorumGateSuppressedPeers?: number
  /** Configured cap on auto-promoted members. */
  quorumGateMaxAutoMembers?: number
  /**
   * True when the latest candidate quorum set failed the bth-quorum-sim
   * intersection check and was refused (the node kept its previous safe set).
   */
  quorumGateIntersectionRefused?: boolean
  /** Peers participating in SCP consensus. */
  scpPeerCount?: number
  /**
   * Live connected peers. `undefined` means `network_getPeers` failed — the
   * table renders an explicit unavailable state, NOT an empty list.
   */
  peers?: TrustPeer[]
}

/**
 * Per-peer gate classification from the operator-only `operator_getQuorumInfo`
 * (#707). `undefined` (not empty arrays) when the node has not yet run a gate
 * evaluation — the node reports `perPeer: null` and we preserve that "no data"
 * distinction (anti-#541).
 */
export interface PerPeerClassification {
  /** Connected curated (operator-listed) members admitted by the gate. */
  curated: string[]
  /** Connected auto-discovered peers promoted into the quorum set. */
  auto: string[]
  /** Connected non-curated peers the gate is keeping OUT of the quorum. */
  suppressed: string[]
}

/**
 * Configured `[network.quorum]` contents for one node, from the operator-only
 * `operator_getQuorumInfo` (#707). These are the gate's INPUTS the public
 * surface does not expose.
 */
export interface OperatorQuorumInfo {
  mode: string
  faultModel: string
  threshold: number
  /** Operator-curated member PeerId strings. */
  members: string[]
  minPeers: number
  maxAutoMembers: number
  /** Per-peer classification, or `undefined` until the first gate evaluation. */
  perPeer?: PerPeerClassification
}

/**
 * Result of an operator-only fetch. Distinguishes the three states the
 * token-gated read can be in, so the UI degrades correctly:
 *   - `not-enabled`: the node has no `[rpc.operator]` config — operator reads
 *     are impossible here; render the public view without an "expired" nag.
 *   - `unauthorized`: token missing / rejected / expired — prompt for a link.
 *   - `unreachable`: the call itself failed (transport/timeout).
 */
export type OperatorFetchResult<T> =
  | { status: 'ok'; data: T }
  | { status: 'not-enabled' }
  | { status: 'unauthorized' }
  | { status: 'unreachable' }

/** Fleet-level trust facts derived from the live snapshots (pure function). */
export interface TrustSummary {
  /** Reachable node count. */
  nodesReachable: number
  /** Total nodes watched. */
  nodesTotal: number
  /** Reachable node ids whose latest gate candidate was intersection-refused. */
  intersectionRefusedNodeIds: string[]
  /** Reachable node ids reporting a degenerate (zero-fault-tolerance) quorum. */
  degenerateNodeIds: string[]
  /** Reachable nodes reporting `quorumFaultTolerant: true`. */
  faultTolerantCount: number
}
