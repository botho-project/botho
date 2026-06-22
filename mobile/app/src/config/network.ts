/**
 * Network identity expectations for the thin client.
 *
 * A thin wallet (this app) is a Mode-2 client: it points at *some* node and
 * trusts that node for RPC ingress. Before trusting a node the app must verify
 * that the node belongs to the network the wallet expects and speaks a
 * compatible wire protocol. This module holds those expectations and the pure
 * comparison helpers used by the node-selection / trust UX (epic #441 P3).
 *
 * Keeping these as plain constants + pure functions makes the trust decision
 * unit-testable without any native bridge or network access.
 */

/**
 * The network this build of the wallet is for.
 *
 * The node's `node_getIdentity` reports `network` as `"botho-<name>"`
 * (e.g. `"botho-testnet"` / `"botho-mainnet"`), so this must match exactly.
 * This is currently a testnet build; flipping it to mainnet is the single
 * switch a mainnet build flips.
 */
export const EXPECTED_NETWORK = "botho-testnet";

/**
 * Wire-protocol version this client speaks. The node advertises its own
 * `protocolVersion` plus the `minProtocolVersion` it will accept from peers;
 * we treat a node as compatible when our version is within that node's
 * accepted window (major version match, see `isProtocolCompatible`).
 */
export const CLIENT_PROTOCOL_VERSION = "2.0.0";

/** Outcome of comparing a node's reported network against this build. */
export type NetworkMatch = "match" | "mismatch" | "unknown";

/**
 * Compare a node-reported network string against the build's expected network.
 *
 * Returns `"unknown"` when the node did not report a network (older node or a
 * field we couldn't parse) so the UI can warn rather than hard-block.
 */
export function compareNetwork(reported: string | undefined | null): NetworkMatch {
  if (reported == null || reported.trim() === "") return "unknown";
  return reported.trim() === EXPECTED_NETWORK ? "match" : "mismatch";
}

/** Parse the leading integer (major) component of a dotted version string. */
function majorOf(version: string): number | null {
  const head = version.trim().split(".")[0];
  if (head === "") return null;
  const n = Number.parseInt(head, 10);
  return Number.isFinite(n) ? n : null;
}

/**
 * Decide whether this client's protocol version is compatible with a node.
 *
 * Compatibility rule (conservative, matches the node's own major-version gate):
 * the client major version must be >= the node's advertised `minProtocolVersion`
 * major and <= the node's advertised `protocolVersion` major. Missing/unparsable
 * fields are treated as "unknown" -> not provably compatible, so the UI warns.
 */
export function isProtocolCompatible(
  nodeProtocolVersion: string | undefined | null,
  nodeMinProtocolVersion: string | undefined | null
): boolean {
  const clientMajor = majorOf(CLIENT_PROTOCOL_VERSION);
  if (clientMajor == null) return false;

  const nodeMajor = nodeProtocolVersion ? majorOf(nodeProtocolVersion) : null;
  if (nodeMajor == null) return false;

  // If the node reports a minimum it accepts, honor it; otherwise assume the
  // node accepts down to its own major.
  const nodeMinMajor = nodeMinProtocolVersion
    ? majorOf(nodeMinProtocolVersion)
    : null;
  const minMajor = nodeMinMajor ?? nodeMajor;

  return clientMajor >= minMajor && clientMajor <= nodeMajor;
}
