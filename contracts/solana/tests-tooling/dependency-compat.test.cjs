"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");
const anchorRequire = createRequire(require.resolve("@coral-xyz/anchor"));
const web3Require = createRequire(require.resolve("@solana/web3.js"));
test("actual Anchor parser handles config and rejects advisory payloads", () => {
  const toml = anchorRequire("toml");
  assert.equal(anchorRequire("toml/package.json").version, "4.2.0");
  const config = toml.parse(
    fs.readFileSync(path.join(__dirname, "../Anchor.toml"), "utf8"),
  );
  assert.equal(config.provider.cluster, "Localnet");
  assert.match(config.scripts.test, /mocha --import=tsx/);
  assert.throws(() =>
    toml.parse("x=" + "[".repeat(1500) + "0" + "]".repeat(1500)),
  );
  assert.throws(() =>
    toml.parse('[a.b]\ny=1\n[a.b.y.__proto__.__proto__]\npolluted="yes"'),
  );
  assert.equal({}.polluted, undefined);
});
test("actual web3 client success/error and unique JSON-RPC IDs", async () => {
  assert.equal(web3Require("jayson/package.json").version, "5.0.0");
  const { Connection, PublicKey } = require("@solana/web3.js");
  const seen = new Set();
  const connection = new Connection("http://127.0.0.1:1", {
    fetch: async (_url, options) => {
      const request = JSON.parse(options.body);
      assert.match(
        request.id,
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i,
      );
      assert(!seen.has(request.id));
      seen.add(request.id);
      const payload =
        seen.size === 1
          ? { result: { context: { slot: 1 }, value: 42 } }
          : { error: { code: -32602, message: "test rejection" } };
      return new Response(
        JSON.stringify({ jsonrpc: "2.0", id: request.id, ...payload }),
        { status: 200 },
      );
    },
  });
  const key = new PublicKey("11111111111111111111111111111111");
  assert.equal(await connection.getBalance(key), 42);
  await assert.rejects(connection.getBalance(key), /test rejection/);
  assert.equal(seen.size, 2);
});
test("jayson browser client retains batch result/error dispatch", async () => {
  const Client = web3Require("jayson/lib/client/browser");
  const client = new Client((body, callback) =>
    callback(
      null,
      JSON.stringify(
        JSON.parse(body).map((r, i) =>
          i === 0
            ? { jsonrpc: "2.0", id: r.id, result: 42 }
            : {
                jsonrpc: "2.0",
                id: r.id,
                error: { code: -32602, message: "rejected" },
              },
        ),
      ),
    ),
  );
  await new Promise((resolve, reject) =>
    client.request(
      [client.request("first", []), client.request("second", [])],
      (err, responses) => {
        try {
          assert.ifError(err);
          assert.equal(responses[0].result, 42);
          assert.equal(responses[1].error.code, -32602);
          resolve();
        } catch (e) {
          reject(e);
        }
      },
    ),
  );
});
test("SPL mint/account maximum u64 and malformed remote data", () => {
  const spl = require("@solana/spl-token");
  const { PublicKey } = require("@solana/web3.js");
  const key = new PublicKey("11111111111111111111111111111111");
  const info = {
    data: Buffer.alloc(spl.MINT_SIZE),
    owner: spl.TOKEN_PROGRAM_ID,
    executable: false,
    lamports: 1,
    rentEpoch: 0,
  };
  info.data.writeBigUInt64LE((1n << 64n) - 1n, 36);
  assert.equal(spl.unpackMint(key, info).supply, (1n << 64n) - 1n);
  const account = { ...info, data: Buffer.alloc(spl.ACCOUNT_SIZE) };
  account.data.writeBigUInt64LE((1n << 64n) - 1n, 64);
  assert.equal(spl.unpackAccount(key, account).amount, (1n << 64n) - 1n);
  for (const n of [0, 1, spl.MINT_SIZE - 1])
    assert.throws(() =>
      spl.unpackMint(key, { ...info, data: Buffer.alloc(n) }),
    );
  for (const n of [0, 1, spl.ACCOUNT_SIZE - 1])
    assert.throws(() =>
      spl.unpackAccount(key, { ...account, data: Buffer.alloc(n) }),
    );
});
