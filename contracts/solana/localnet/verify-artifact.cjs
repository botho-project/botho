const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { createHash } = require("node:crypto");
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const dir = "fixtures/squads-v4";
const binary = readFileSync(`${dir}/squads_multisig_program.so`);
assert.equal(
  sha(binary),
  "dec8d3e0fae58c7c8f2416e5f67c25e673f047afd6dd2bba4a47e0b29a01d34c",
);
let end = binary.length;
while (end > 0 && binary[end - 1] === 0) end--;
assert.equal(
  sha(binary.subarray(0, end)),
  "d48660833989ecea3145ff726164fe640bd90696f03ce00dfd0cda258cbf2fac",
);
assert.equal(
  sha(readFileSync(`${dir}/idl.json`)),
  "cb9a0a29040ec3853a5105c547ffc608bb0e3175d78dacf30d463aff939f9b9b",
);
console.log("Pinned Squads artifact and IDL hashes verified");
