/**
 * Node identity probe (app-side JSON-RPC, no native bridge).
 *
 * Trust UX for thin clients (epic #441 P3): before the user marks a node as
 * trusted and points the wallet's RPC ingress at it, the app fetches the
 * node's verifiable identity via the `node_getIdentity` RPC (#500) and shows it
 * for confirmation.
 *
 * This deliberately talks to the candidate node over plain JSON-RPC HTTP
 * instead of going through the rust-bridge `setNodeUrl` -> bridge-call path:
 *
 *   1. Identity verification must run against an *un-trusted candidate* URL.
 *      Repointing the shared native bridge at an unverified node just to read
 *      its identity would be a footgun (subsequent bridge ops could hit it).
 *   2. `node_getIdentity` is a public, unauthenticated, read-only endpoint, so
 *      a thin `fetch` is sufficient and keeps this entirely in the app layer
 *      (the rust-bridge does not yet expose a getNodeIdentity method).
 *
 * Only the active/selected node is ever handed to the bridge via `setNodeUrl`.
 */

import type { NodeIdentity } from "../types/wallet";

/** How long to wait for a candidate node before treating it as unreachable. */
const IDENTITY_TIMEOUT_MS = 8000;

/** JSON-RPC method that returns the node's verifiable identity (#500). */
const IDENTITY_METHOD = "node_getIdentity";

/** Error thrown when a node cannot be probed or returns a bad response. */
export class NodeIdentityError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "NodeIdentityError";
  }
}

/** Normalize a user-entered URL into an RPC base (https default, no trailing /). */
export function normalizeNodeUrl(raw: string): string {
  let url = raw.trim();
  if (url === "") {
    throw new NodeIdentityError("Enter a node URL.");
  }
  // Default to https:// when the user omits a scheme (thin clients should not
  // talk to nodes over plaintext http).
  if (!/^https?:\/\//i.test(url)) {
    url = `https://${url}`;
  }
  // Drop trailing slashes so we can append the RPC path deterministically.
  url = url.replace(/\/+$/, "");

  try {
    // Validate it parses as a URL; throws for garbage input.
    // eslint-disable-next-line no-new
    new URL(url);
  } catch {
    throw new NodeIdentityError(`"${raw}" is not a valid URL.`);
  }
  return url;
}

/** Coerce a JSON value that may be a number or numeric string to a number. */
function toNumber(value: unknown): number {
  if (typeof value === "number") return value;
  if (typeof value === "string") {
    const n = Number(value);
    return Number.isFinite(n) ? n : 0;
  }
  return 0;
}

/** Coerce a JSON value to a string (empty when absent). */
function toStr(value: unknown): string {
  return typeof value === "string" ? value : value == null ? "" : String(value);
}

/** Map the raw `node_getIdentity` result object onto the typed `NodeIdentity`. */
function adaptIdentity(result: Record<string, unknown>): NodeIdentity {
  return {
    peerId: toStr(result.peerId),
    nodeId: toStr(result.nodeId),
    network: toStr(result.network),
    protocolVersion: toStr(result.protocolVersion),
    minProtocolVersion: toStr(result.minProtocolVersion),
    nodeVersion: toStr(result.nodeVersion ?? result.version),
    gitCommit: toStr(result.gitCommit),
    dnsSeedDomain: toStr(result.dnsSeedDomain),
    chainHeight: toNumber(result.chainHeight),
    tipHash: toStr(result.tipHash),
  };
}

/**
 * Fetch and verify a node's identity over JSON-RPC.
 *
 * @param rawUrl user-entered node URL (scheme optional).
 * @returns the parsed {@link NodeIdentity}.
 * @throws {NodeIdentityError} on bad URL, timeout, transport error, JSON-RPC
 *   error, or a node that does not implement `node_getIdentity`.
 */
export async function fetchNodeIdentity(rawUrl: string): Promise<NodeIdentity> {
  const url = normalizeNodeUrl(rawUrl);

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), IDENTITY_TIMEOUT_MS);

  let response: Response;
  try {
    response = await fetch(`${url}/rpc`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        method: IDENTITY_METHOD,
        params: {},
        id: 1,
      }),
      signal: controller.signal,
    });
  } catch (error) {
    clearTimeout(timer);
    const aborted = error instanceof Error && error.name === "AbortError";
    throw new NodeIdentityError(
      aborted
        ? "Node did not respond in time."
        : `Could not reach ${url}. Check the URL and your connection.`
    );
  }
  clearTimeout(timer);

  if (!response.ok) {
    throw new NodeIdentityError(
      `Node responded with HTTP ${response.status}.`
    );
  }

  let body: unknown;
  try {
    body = await response.json();
  } catch {
    throw new NodeIdentityError("Node returned a non-JSON response.");
  }

  if (typeof body !== "object" || body === null) {
    throw new NodeIdentityError("Node returned an unexpected response.");
  }

  const rpc = body as { error?: { message?: string }; result?: unknown };
  if (rpc.error) {
    throw new NodeIdentityError(
      rpc.error.message ??
        "Node does not support identity verification (node_getIdentity)."
    );
  }
  if (typeof rpc.result !== "object" || rpc.result === null) {
    throw new NodeIdentityError(
      "Node did not return an identity. It may be running an older version."
    );
  }

  const identity = adaptIdentity(rpc.result as Record<string, unknown>);
  if (identity.peerId === "" && identity.network === "") {
    throw new NodeIdentityError(
      "Node returned an empty identity. It may not be a Botho node."
    );
  }
  return identity;
}
