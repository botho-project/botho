// Throughput of decaps / verify for a wasm32 probe build inside Node.
//   node bench.mjs <probe.wasm> <kat-dir>
import { readFileSync } from "node:fs";
import { join } from "node:path";

const [wasmPath, katDir] = process.argv.slice(2);
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
const x = instance.exports;
const base = x.probe_buf();
const put = (off, bytes) => new Uint8Array(x.memory.buffer).set(bytes, base + off);
const hex = (s) => Uint8Array.from(Buffer.from(s, "hex"));
const load = (n) => JSON.parse(readFileSync(join(katDir, `${n}.json`), "utf8"));

const kem = load("ML-KEM-encapDecap-FIPS203").testGroups.find(
  (g) => g.parameterSet === "ML-KEM-768" && g.function === "decapsulation").tests[0];
put(0, hex(kem.dk)); put(3000, hex(kem.c));

const dsa = load("ML-DSA-sigVer-FIPS204").testGroups.find(
  (g) => g.parameterSet === "ML-DSA-65" && g.signatureInterface === "external" && g.preHash === "pure")
  .tests.find((t) => t.testPassed);
put(10000, hex(dsa.pk)); put(14000, hex(dsa.signature));
const msg = hex(dsa.message); put(20000, msg);
const ctx = hex(dsa.context); put(60000, ctx);

function time(name, f) {
  for (let i = 0; i < 300; i++) f();
  const t0 = process.hrtime.bigint();
  let n = 0;
  while (Number(process.hrtime.bigint() - t0) < 3e9) { for (let i = 0; i < 100; i++) f(); n += 100; }
  const s = Number(process.hrtime.bigint() - t0) / 1e9;
  console.log(`${name.padEnd(28)} ${(n / s).toFixed(0).padStart(8)} ops/s ${(1e6 * s / n).toFixed(2).padStart(8)} µs/op`);
}
console.log(wasmPath);
time("ML-KEM-768 decaps", () => x.probe_decaps(base, base + 3000, base + 6000));
time("ML-DSA-65 verify", () => {
  if (!x.probe_verify(base + 10000, base + 20000, msg.length, base + 60000, ctx.length, base + 14000)) throw new Error("verify");
});
