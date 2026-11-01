// Parser for `pnpm why -r <--prod|--dev> <pkg> --json`, used by
// check-elliptic-containment.mjs (#1562).
//
// pnpm has shipped two different JSON shapes for this command:
//
//   pnpm 9 ("importer" shape): one entry per workspace importer,
//     [{ name, version, path, private, dependencies?, devDependencies?,
//        optionalDependencies? }, ...]
//     Importers that do not reach the package carry no dependency map.
//
//   pnpm >= 10 ("dependents" shape): one entry per installed copy of the
//     package, with a reverse tree that ends at workspace importers,
//     [{ name: <pkg>, version, path, dependents: [{ name, version,
//        dependents?: [...], depField?, deduped? }, ...] }]
//     An importer leaf carries `depField` ("dependencies",
//     "devDependencies", ...). A `deduped` leaf was already expanded
//     elsewhere in the tree.
//
// The parser FAILS CLOSED. Output it cannot recognise throws
// UnrecognizedWhyOutput instead of returning "no importers". If it returned
// an empty result, a pnpm upgrade could silently disable the containment
// checks.

export class UnrecognizedWhyOutput extends Error {
  constructor(msg) {
    super(`unrecognized \`pnpm why --json\` output: ${msg}`);
    this.name = 'UnrecognizedWhyOutput';
  }
}

const DEP_MAP_KEYS = ['dependencies', 'devDependencies', 'optionalDependencies', 'peerDependencies'];
const isObj = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);

/**
 * Parse `pnpm why --json` output.
 *
 * @param {unknown} json  parsed JSON (or '' / undefined for empty stdout)
 * @param {string} pkg    package that was asked about (e.g. 'elliptic')
 * @returns {{ shape: 'empty' | 'importers' | 'dependents',
 *             importers: Map<string, Set<string>> }}
 *   importers maps each workspace importer that reaches `pkg` to the set of
 *   dependency fields it reaches it through.
 */
export function parseWhy(json, pkg) {
  const importers = new Map();
  const add = (name, field) => {
    if (typeof name !== 'string' || name === '') {
      throw new UnrecognizedWhyOutput('importer without a name');
    }
    if (!importers.has(name)) importers.set(name, new Set());
    importers.get(name).add(field);
  };

  if (json === undefined || json === null || json === '') return { shape: 'empty', importers };
  if (!Array.isArray(json)) throw new UnrecognizedWhyOutput('top level is not an array');
  if (json.length === 0) return { shape: 'empty', importers };
  if (!json.every(isObj)) throw new UnrecognizedWhyOutput('array entry is not an object');

  const dependentsShape = json.some((e) => 'dependents' in e);
  if (dependentsShape) {
    for (const e of json) {
      if (e.name !== pkg) {
        throw new UnrecognizedWhyOutput(`top-level entry "${e.name}" is not ${pkg}`);
      }
      if (!Array.isArray(e.dependents)) {
        throw new UnrecognizedWhyOutput(`${pkg} entry has no dependents array`);
      }
      const walk = (node) => {
        if (!isObj(node) || typeof node.name !== 'string') {
          throw new UnrecognizedWhyOutput('dependent node without a name');
        }
        if (typeof node.depField === 'string') add(node.name, node.depField);
        if (Array.isArray(node.dependents) && node.dependents.length > 0) {
          node.dependents.forEach(walk);
        } else if (typeof node.depField !== 'string' && node.deduped !== true) {
          // A leaf that is neither an importer nor a deduped reference is a
          // shape this parser was not written for.
          throw new UnrecognizedWhyOutput(`leaf "${node.name}" is neither an importer nor deduped`);
        }
      };
      e.dependents.forEach(walk);
    }
    return { shape: 'dependents', importers };
  }

  for (const e of json) {
    if (typeof e.name !== 'string' || typeof e.path !== 'string') {
      throw new UnrecognizedWhyOutput('importer entry without name/path');
    }
    for (const k of Object.keys(e)) {
      if (!['name', 'version', 'path', 'private', ...DEP_MAP_KEYS].includes(k)) {
        throw new UnrecognizedWhyOutput(`importer entry has unexpected key "${k}"`);
      }
    }
    for (const k of DEP_MAP_KEYS) {
      if (e[k] === undefined) continue;
      if (!isObj(e[k])) throw new UnrecognizedWhyOutput(`"${k}" of ${e.name} is not an object`);
      if (Object.keys(e[k]).length > 0) add(e.name, k);
    }
  }
  return { shape: 'importers', importers };
}
