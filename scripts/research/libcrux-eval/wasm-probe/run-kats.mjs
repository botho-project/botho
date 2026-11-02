// Run the ML-KEM-768 encaps/decaps and ML-DSA-65 sigVer ACVP vectors
// against the wasm32 build of the probe, inside Node's WebAssembly engine.
//
//   node run-kats.mjs <probe.wasm> <kat-dir>
import { readFileSync } from "node:fs";
import { join } from "node:path";

const [wasmPath, katDir] = process.argv.slice(2);
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
const x = instance.exports;
const base = x.probe_buf();
const mem = () => new Uint8Array(x.memory.buffer);
const hex = (s) => Uint8Array.from(Buffer.from(s, "hex"));
const put = (off, bytes) => mem().set(bytes, base + off);
const get = (off, n) => Buffer.from(mem().slice(base + off, base + off + n)).toString("hex");

const load = (n) => JSON.parse(readFileSync(join(katDir, `${n}.json`), "utf8"));
let enc = 0, dec = 0, ver = 0, fail = 0;

for (const g of load("ML-KEM-encapDecap-FIPS203").testGroups) {
  if (g.parameterSet !== "ML-KEM-768") continue;
  for (const t of g.tests) {
    if (g.function === "encapsulation") {
      put(0, hex(t.ek)); put(2000, hex(t.m));
      x.probe_encaps(base, base + 2000, base + 4000, base + 6000);
      if (get(4000, 1088) !== t.c.toLowerCase() || get(6000, 32) !== t.k.toLowerCase()) fail++;
      enc++;
    } else if (g.function === "decapsulation") {
      put(0, hex(t.dk)); put(3000, hex(t.c));
      x.probe_decaps(base, base + 3000, base + 6000);
      if (get(6000, 32) !== t.k.toLowerCase()) fail++;
      dec++;
    }
  }
}
for (const g of load("ML-DSA-sigVer-FIPS204").testGroups) {
  if (g.parameterSet !== "ML-DSA-65" || g.signatureInterface !== "external" || g.preHash !== "pure") continue;
  for (const t of g.tests) {
    put(0, hex(t.pk)); put(4000, hex(t.signature));
    const msg = hex(t.message); put(8000, msg);
    const ctx = hex(t.context); put(60000, ctx);
    const ok = x.probe_verify(base, base + 8000, msg.length, base + 60000, ctx.length, base + 4000);
    if (Boolean(ok) !== t.testPassed) fail++;
    ver++;
  }
}
console.log(`wasm32 KATs: ML-KEM-768 encaps ${enc - 0}, decaps ${dec}, ML-DSA-65 sigVer (external/pure) ${ver}; failures=${fail}`);
process.exit(fail ? 1 : 0);
