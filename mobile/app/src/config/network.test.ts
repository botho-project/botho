/**
 * Tests for the thin-client network trust comparisons (epic #441 P3).
 */

import {
  EXPECTED_NETWORK,
  compareNetwork,
  isProtocolCompatible,
} from "./network";

describe("compareNetwork", () => {
  it("matches the expected network exactly", () => {
    expect(compareNetwork(EXPECTED_NETWORK)).toBe("match");
  });

  it("trims surrounding whitespace before comparing", () => {
    expect(compareNetwork(`  ${EXPECTED_NETWORK}  `)).toBe("match");
  });

  it("flags a different network as a mismatch", () => {
    expect(compareNetwork("botho-mainnet")).toBe("mismatch");
  });

  it("treats missing / empty networks as unknown", () => {
    expect(compareNetwork(undefined)).toBe("unknown");
    expect(compareNetwork(null)).toBe("unknown");
    expect(compareNetwork("")).toBe("unknown");
    expect(compareNetwork("   ")).toBe("unknown");
  });
});

describe("isProtocolCompatible", () => {
  it("accepts a node whose major version matches the client", () => {
    // CLIENT_PROTOCOL_VERSION is 4.x (issue #605/#626 reset: log-domain fee
    // curve, u128 cluster wealth); node speaks 4.x and accepts down to 4.x.
    expect(isProtocolCompatible("4.0.0", "4.0.0")).toBe(true);
    expect(isProtocolCompatible("4.3.1", "4.0.0")).toBe(true);
  });

  it("accepts when the client falls inside the node's accepted window", () => {
    // Node speaks 5.x but accepts down to 4.x -> client 4.x is fine.
    expect(isProtocolCompatible("5.0.0", "4.0.0")).toBe(true);
  });

  it("rejects when the node requires a newer protocol than the client", () => {
    expect(isProtocolCompatible("5.0.0", "5.0.0")).toBe(false);
  });

  it("rejects when the node is older than the client", () => {
    // A pre-reset 3.x node (old C7 fee floor) no longer shares consensus
    // rules with this client.
    expect(isProtocolCompatible("3.0.0", "3.0.0")).toBe(false);
    expect(isProtocolCompatible("2.0.0", "2.0.0")).toBe(false);
    expect(isProtocolCompatible("1.0.0", "1.0.0")).toBe(false);
  });

  it("rejects when the node's protocol version is missing/unparsable", () => {
    expect(isProtocolCompatible(undefined, undefined)).toBe(false);
    expect(isProtocolCompatible("", "")).toBe(false);
    expect(isProtocolCompatible("abc", "4.0.0")).toBe(false);
  });

  it("defaults the min to the node's own major when min is absent", () => {
    expect(isProtocolCompatible("4.0.0", undefined)).toBe(true);
    expect(isProtocolCompatible("5.0.0", undefined)).toBe(false);
  });
});
