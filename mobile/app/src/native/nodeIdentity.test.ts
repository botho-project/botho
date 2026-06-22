/**
 * Tests for the node-identity URL normalizer (epic #441 P3).
 *
 * Only the pure URL-normalization logic is unit-tested here; the network
 * `fetch` path is exercised at integration / runtime.
 */

import { normalizeNodeUrl, NodeIdentityError } from "./nodeIdentity";

describe("normalizeNodeUrl", () => {
  it("keeps a well-formed https URL", () => {
    expect(normalizeNodeUrl("https://node.example.com")).toBe(
      "https://node.example.com"
    );
  });

  it("defaults a scheme-less host to https", () => {
    expect(normalizeNodeUrl("node.example.com")).toBe(
      "https://node.example.com"
    );
  });

  it("preserves an explicit http scheme", () => {
    expect(normalizeNodeUrl("http://10.0.0.1:8332")).toBe(
      "http://10.0.0.1:8332"
    );
  });

  it("strips trailing slashes", () => {
    expect(normalizeNodeUrl("https://node.example.com///")).toBe(
      "https://node.example.com"
    );
  });

  it("trims surrounding whitespace", () => {
    expect(normalizeNodeUrl("  https://node.example.com  ")).toBe(
      "https://node.example.com"
    );
  });

  it("throws on empty input", () => {
    expect(() => normalizeNodeUrl("")).toThrow(NodeIdentityError);
    expect(() => normalizeNodeUrl("   ")).toThrow(NodeIdentityError);
  });

  it("throws on an unparsable URL", () => {
    expect(() => normalizeNodeUrl("https://")).toThrow(NodeIdentityError);
  });
});
