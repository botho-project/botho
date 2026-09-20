"use strict";
const fs = require("node:fs");
const path = require("node:path");
const { createHash } = require("node:crypto");

// @solana/buffer-layout 4.0.1's OffsetLayout declaration has the correct extends
// clause but an incorrect JSDoc base-class tag. TypeScript 7 reports TS8023.
// Correct that comment only; preserve all signatures, MIT notice and runtime JS.
// See ../TYPECHECK.md for provenance and removal/update conditions.
const ORIGINAL =
  "92ad95e6220ab829c8f5cfca9be43e26e041a2922cde6e998782030d41c49963";
const CORRECTED =
  "294f75913b809804d9f3b3272ed5471b0be5b4aa225e6b912b775af4032fc303";
const hash = (content) => createHash("sha256").update(content).digest("hex");

function prepareBufferLayoutTypes(
  packageDirectory = path.dirname(
    require.resolve("@solana/buffer-layout/package.json"),
  ),
) {
  const pkg = JSON.parse(
    fs.readFileSync(path.join(packageDirectory, "package.json"), "utf8"),
  );
  if (pkg.name !== "@solana/buffer-layout" || pkg.version !== "4.0.1") {
    throw new Error(
      "unreviewed package: re-evaluate the buffer-layout typecheck correction",
    );
  }
  const file = path.join(packageDirectory, "lib/Layout.d.ts");
  const original = fs.readFileSync(file);
  const digest = hash(original);
  if (digest === CORRECTED) return false;
  if (digest !== ORIGINAL)
    throw new Error(
      "unrecognized declaration: refusing to patch buffer-layout types",
    );
  const oldTag =
    " * @augments {Layout}\n */\nexport declare class OffsetLayout extends ExternalLayout";
  const newTag = oldTag.replace(
    "@augments {Layout}",
    "@augments {ExternalLayout}",
  );
  const text = original.toString("utf8");
  if (text.split(oldTag).length !== 2)
    throw new Error("expected one OffsetLayout JSDoc tag");
  const corrected = text.replace(oldTag, newTag);
  if (hash(corrected) !== CORRECTED)
    throw new Error("unexpected corrected declaration hash");
  fs.writeFileSync(file, corrected);
  return true;
}

module.exports = { prepareBufferLayoutTypes };
if (require.main === module) {
  console.log(
    prepareBufferLayoutTypes()
      ? "Corrected buffer-layout inheritance JSDoc for typecheck"
      : "Buffer-layout inheritance JSDoc already corrected",
  );
}
