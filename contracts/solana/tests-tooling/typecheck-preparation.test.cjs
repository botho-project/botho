"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { createHash } = require("node:crypto");
const {
  prepareBufferLayoutTypes,
} = require("../scripts/prepare-typecheck.cjs");
const originalHash =
  "92ad95e6220ab829c8f5cfca9be43e26e041a2922cde6e998782030d41c49963";
const patchedHash =
  "294f75913b809804d9f3b3272ed5471b0be5b4aa225e6b912b775af4032fc303";
const hash = (value) => createHash("sha256").update(value).digest("hex");
const oldTag =
  " * @augments {Layout}\n */\nexport declare class OffsetLayout extends ExternalLayout";
const newTag = oldTag.replace(
  "@augments {Layout}",
  "@augments {ExternalLayout}",
);
function fixture(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "botho-typecheck-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  fs.mkdirSync(path.join(dir, "lib"));
  const installed = path.dirname(
    require.resolve("@solana/buffer-layout/package.json"),
  );
  const text = fs
    .readFileSync(path.join(installed, "lib/Layout.d.ts"), "utf8")
    .replace(newTag, oldTag);
  assert.equal(hash(text), originalHash);
  fs.writeFileSync(
    path.join(dir, "package.json"),
    JSON.stringify({ name: "@solana/buffer-layout", version: "4.0.1" }),
  );
  fs.writeFileSync(path.join(dir, "lib/Layout.d.ts"), text);
  fs.copyFileSync(
    path.join(installed, "lib/Layout.js"),
    path.join(dir, "lib/Layout.js"),
  );
  return dir;
}
test("preparation corrects only known inheritance JSDoc, is idempotent, leaves runtime intact", (t) => {
  const dir = fixture(t),
    declaration = path.join(dir, "lib/Layout.d.ts"),
    runtime = path.join(dir, "lib/Layout.js");
  const before = fs.readFileSync(declaration, "utf8"),
    jsBefore = fs.readFileSync(runtime);
  assert.equal(prepareBufferLayoutTypes(dir), true);
  const after = fs.readFileSync(declaration, "utf8");
  assert.equal(after, before.replace(oldTag, newTag));
  assert.equal(hash(after), patchedHash);
  assert.equal(prepareBufferLayoutTypes(dir), false);
  assert.deepEqual(fs.readFileSync(runtime), jsBefore);
});
test("preparation rejects unexpected declaration bytes without modifying them", (t) => {
  const dir = fixture(t),
    declaration = path.join(dir, "lib/Layout.d.ts");
  fs.appendFileSync(declaration, "\n// unexpected change\n");
  const before = fs.readFileSync(declaration);
  assert.throws(
    () => prepareBufferLayoutTypes(dir),
    /unrecognized declaration/,
  );
  assert.deepEqual(fs.readFileSync(declaration), before);
});
test("preparation rejects an unreviewed version even if declaration bytes match", (t) => {
  const dir = fixture(t),
    declaration = path.join(dir, "lib/Layout.d.ts");
  fs.writeFileSync(
    path.join(dir, "package.json"),
    JSON.stringify({ name: "@solana/buffer-layout", version: "4.0.2" }),
  );
  const before = fs.readFileSync(declaration);
  assert.throws(() => prepareBufferLayoutTypes(dir), /unreviewed package/);
  assert.deepEqual(fs.readFileSync(declaration), before);
});
