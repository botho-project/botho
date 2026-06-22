/**
 * Tests for the user-managed node-list helpers (epic #441 P3).
 */

import {
  seedNodes,
  TESTNET_NODES,
  findManagedByUrl,
  nodeIdForUrl,
  labelFromUrl,
} from "./nodes";

describe("seedNodes", () => {
  it("returns one managed entry per testnet node, marked as a seed", () => {
    const seeds = seedNodes();
    expect(seeds).toHaveLength(TESTNET_NODES.length);
    for (const node of seeds) {
      expect(node.source).toBe("seed");
    }
  });

  it("preserves the testnet URLs and faucet flag", () => {
    const seeds = seedNodes();
    const faucet = seeds.find((n) => n.isFaucet);
    expect(faucet?.url).toBe("https://faucet.botho.io");
  });

  it("returns a fresh array each call (no shared mutation)", () => {
    const a = seedNodes();
    const b = seedNodes();
    expect(a).not.toBe(b);
    a[0].label = "mutated";
    expect(b[0].label).not.toBe("mutated");
  });
});

describe("findManagedByUrl", () => {
  it("finds a node by exact URL", () => {
    const nodes = seedNodes();
    const found = findManagedByUrl(nodes, "https://seed.botho.io");
    expect(found?.id).toBe("seed");
  });

  it("returns undefined when no node matches", () => {
    expect(findManagedByUrl(seedNodes(), "https://nope.example")).toBeUndefined();
  });
});

describe("nodeIdForUrl", () => {
  it("derives a deterministic id from the URL", () => {
    expect(nodeIdForUrl("https://a.example")).toBe(
      nodeIdForUrl("https://a.example")
    );
    expect(nodeIdForUrl("https://a.example")).not.toBe(
      nodeIdForUrl("https://b.example")
    );
  });
});

describe("labelFromUrl", () => {
  it("uses the host as the default label", () => {
    expect(labelFromUrl("https://node.example.com:8080/rpc")).toBe(
      "node.example.com:8080"
    );
  });

  it("falls back to the raw string for an unparsable URL", () => {
    expect(labelFromUrl("not a url")).toBe("not a url");
  });
});
