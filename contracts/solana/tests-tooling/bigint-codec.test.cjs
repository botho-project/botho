"use strict";
const a = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { createHash } = require("node:crypto");
const BN = require("bn.js");
const fixtures = require("./fixtures/bigint-buffer-1.1.5.json");
const root = path.dirname(require.resolve("bigint-buffer/package.json"));
const nodeCodec = require("bigint-buffer");
const consumerPaths = Object.keys(require("../package-lock.json").packages)
  .filter((name) => name.endsWith("node_modules/@solana/buffer-layout-utils"))
  .map((name) => path.resolve(__dirname, "..", name))
  .filter((name) => fs.existsSync(path.join(name, "package.json")));
a.ok(consumerPaths.length > 0);

const browserBuffer = require("buffer/").Buffer;
// No Buffer global is supplied to the browser entry.
const context = vm.createContext({
  module: { exports: {} },
  require: (name) => {
    if (name === "buffer/") return { Buffer: browserBuffer };
    a.equal(name, "./codec.cjs");
    return require(path.join(root, name));
  },
});
vm.runInContext(
  fs.readFileSync(path.join(root, "browser.cjs"), "utf8"),
  context,
);
a.equal(vm.runInContext("typeof Buffer", context), "undefined");
for (const [name, c, B] of [
  ["Node", nodeCodec, Buffer],
  ["browser", context.module.exports, browserBuffer],
]) {
  test(`${name}: original native/fallback fixtures and independent BN bytes`, () => {
    for (const v of fixtures.vectors)
      for (const [endian, enc, dec] of [
        ["le", "toBufferLE", "toBigIntLE"],
        ["be", "toBufferBE", "toBigIntBE"],
      ]) {
        a.equal(c[enc](BigInt(v.decimal), v.width).toString("hex"), v[endian]);
        a.equal(c[dec](B.from(v[endian], "hex")), BigInt(v.decimal));
      }
    for (const width of [1, 2, 4, 8, 16, 24, 32, 1024]) {
      const max = (1n << BigInt(width * 8)) - 1n;
      const values = [0n, 1n, max, 1n << BigInt(width * 8 - 1)];
      for (let i = 0; i < 32; i++)
        values.push(
          BigInt(
            "0x" +
              createHash("sha256").update(`botho-${width}-${i}`).digest("hex"),
          ) & max,
        );
      for (const n of values)
        for (const [endian, enc, dec] of [
          ["le", "toBufferLE", "toBigIntLE"],
          ["be", "toBufferBE", "toBigIntBE"],
        ]) {
          const bytes = c[enc](n, width),
            before = bytes.toString("hex");
          a.equal(
            before,
            new BN(n.toString())
              .toArrayLike(Buffer, endian, width)
              .toString("hex"),
          );
          a.equal(c[dec](bytes), n);
          a.equal(bytes.toString("hex"), before);
        }
    }
  });
  test(`${name}: rejects malformed inputs, overflow and excessive work`, () => {
    for (const decode of [c.toBigIntLE, c.toBigIntBE]) {
      for (const bad of [null, undefined, "ff", 1, [], new Uint8Array(8), {}])
        a.throws(() => decode(bad), TypeError);
      a.throws(() => decode(B.alloc(1025)), RangeError);
      a.equal(decode(B.alloc(0)), 0n);
      const backing = B.from([7, 1, 2, 3, 9]);
      decode(backing.subarray(1, 4));
      a.equal(backing.toString("hex"), "0701020309");
    }
    for (const encode of [c.toBufferLE, c.toBufferBE]) {
      for (const width of [
        -1,
        0.5,
        NaN,
        Infinity,
        -Infinity,
        1025,
        Number.MAX_SAFE_INTEGER,
        "8",
        null,
        undefined,
      ])
        a.throws(() => encode(0n, width), RangeError);
      for (const bad of [0, "0", null, undefined, {}, new BN(1)])
        a.throws(() => encode(bad, 8), TypeError);
      a.throws(() => encode(-1n, 8), RangeError);
      for (const width of [0, 1, 8, 16, 32, 1024])
        a.throws(() => encode(1n << BigInt(width * 8), width), RangeError);
      a.equal(encode(0n, 0).length, 0);
    }
  });
}
test("actual SPL consumer resolves local package without native/install hooks", () => {
  for (const parent of consumerPaths) {
    a.equal(
      require.resolve("bigint-buffer", { paths: [parent] }),
      require.resolve("bigint-buffer"),
    );
  }
  const pkg = require("bigint-buffer/package.json");
  a.equal(pkg.name, "@botho/bigint-buffer-js");
  a.equal(pkg.scripts, undefined);
  a.deepEqual(pkg.dependencies, { buffer: "6.0.3" });
});
test("SPL unsigned codecs preserve endian bytes, offsets and range boundaries", () => {
  for (const consumer of consumerPaths) {
    const layouts = require(consumer);
    for (const bits of [64, 128, 192, 256])
      for (const suffix of ["", "be"]) {
        const layout = layouts[`u${bits}${suffix}`]();
        for (const n of [0n, 1n, (1n << BigInt(bits)) - 1n]) {
          const out = Buffer.alloc(bits / 8 + 2, 0xab);
          layout.encode(n, out, 1);
          a.equal(layout.decode(out, 1), n);
          a.equal(out[0], 0xab);
          a.equal(out[out.length - 1], 0xab);
          a.equal(
            out.subarray(1, -1).toString("hex"),
            new BN(n.toString())
              .toArrayLike(Buffer, suffix ? "be" : "le", bits / 8)
              .toString("hex"),
          );
        }
        a.throws(
          () => layout.encode(-1n, Buffer.alloc(bits / 8), 0),
          RangeError,
        );
        a.throws(
          () => layout.encode(1n << BigInt(bits), Buffer.alloc(bits / 8), 0),
          RangeError,
        );
      }
  }
});
test("Anchor BN i128 retains signed two's-complement boundaries", () => {
  const borsh = require("@coral-xyz/borsh");
  for (const n of [-(1n << 127n), -1n, 0n, 1n, (1n << 127n) - 1n]) {
    const out = Buffer.alloc(16);
    borsh.i128().encode(new BN(n.toString()), out);
    a.equal(borsh.i128().decode(out).toString(), n.toString());
    a.deepEqual(out, nodeCodec.toBufferLE(n < 0n ? (1n << 128n) + n : n, 16));
  }
});
