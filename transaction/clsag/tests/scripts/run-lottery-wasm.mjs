// Execute the detached test-only Rust cdylib in a real WebAssembly runtime.
// Transitive wasm-bindgen metadata imports are unreachable in this pure path.
// Every import traps if called: no host crypto, randomness, or mocked result.
import { readFileSync } from 'node:fs';
const path = process.argv[2];
if (!path) throw new Error('usage: node run-lottery-wasm.mjs <lottery_v2_wasm.wasm>');
const module = await WebAssembly.compile(readFileSync(path));
const imports = {};
for (const entry of WebAssembly.Module.imports(module)) {
  if (entry.kind !== 'function' || !entry.module.startsWith('__wbindgen_')) {
    throw new Error(`unexpected WASM import ${entry.module}.${entry.name}`);
  }
  imports[entry.module] ??= {};
  imports[entry.module][entry.name] = () => {
    throw new Error(`pure vector path called host import ${entry.module}.${entry.name}`);
  };
}
const instance = await WebAssembly.instantiate(module, imports);
if (instance.exports.verify_lottery_vectors() !== 0) throw new Error('WASM fixture mismatch');
console.log('Actual WASM runtime: committed lottery V2 vectors match (no host imports called)');
