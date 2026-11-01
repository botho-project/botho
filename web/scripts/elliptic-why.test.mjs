// Tests for the `pnpm why --json` parser behind the elliptic containment
// guard (#1562). Run from web/: `node --test scripts/elliptic-why.test.mjs`.
// The fixtures are trimmed from real pnpm 9.15.9 and pnpm 12.16.1 output for
// this workspace.

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { parseWhy, UnrecognizedWhyOutput } from './elliptic-why.mjs';

const dep = (name, version, extra = {}) => ({ from: name, version, path: `/x/${name}`, ...extra });

// pnpm 9: importer-shaped. Importers that do not reach elliptic carry no maps.
const PNPM9_DEV = [
  { name: 'botho-web', version: '0.6.0', path: '/w', private: true },
  { name: '@botho/web-wallet', version: '0.1.0', path: '/w/packages/web-wallet', private: true },
  {
    name: '@botho/snap',
    version: '0.1.0',
    path: '/w/packages/snap',
    private: true,
    devDependencies: {
      '@metamask/snaps-cli': dep('@metamask/snaps-cli', '8.4.1', {
        dependencies: { 'crypto-browserify': dep('crypto-browserify', '3.12.1') },
      }),
    },
  },
  {
    name: '@botho/snap-spike',
    version: '0.0.1',
    path: '/w/packages/snap-spike',
    private: true,
    devDependencies: { '@metamask/snaps-jest': dep('@metamask/snaps-jest', '10.2.1') },
  },
];

const PNPM9_PROD_NONE = PNPM9_DEV.map(({ devDependencies, ...rest }) => rest);

const PNPM9_PROD_LEAK = [
  ...PNPM9_PROD_NONE.filter((e) => e.name !== '@botho/web-wallet'),
  {
    name: '@botho/web-wallet',
    version: '0.1.0',
    path: '/w/packages/web-wallet',
    private: true,
    dependencies: { '@botho/ui': dep('@botho/ui', 'link:../ui') },
  },
];

// pnpm >= 10: reverse "dependents" tree ending at importer leaves.
const PNPM12_DEV = [
  {
    name: 'elliptic',
    version: '6.6.1',
    path: '/w/node_modules/.pnpm/elliptic@6.6.1/node_modules/elliptic',
    dependents: [
      {
        name: '@ethersproject/signing-key',
        version: '5.8.0',
        dependents: [
          {
            name: '@metamask/snaps-jest',
            version: '10.2.1',
            peersSuffixHash: '1313',
            dependents: [
              { name: '@botho/snap', version: '0.1.0', depField: 'devDependencies' },
              { name: '@botho/snap-spike', version: '0.0.1', depField: 'devDependencies' },
            ],
          },
        ],
      },
      {
        name: 'crypto-browserify',
        version: '3.12.1',
        dependents: [{ name: '@metamask/snaps-cli', version: '8.4.1', peersSuffixHash: 'ab12', deduped: true }],
      },
    ],
  },
];

const PNPM12_PROD_LEAK = [
  {
    name: 'elliptic',
    version: '6.6.1',
    path: '/w/node_modules/.pnpm/elliptic@6.6.1/node_modules/elliptic',
    dependents: [
      {
        name: '@botho/ui',
        version: '0.1.0',
        depField: 'dependencies',
        dependents: [{ name: '@botho/web-wallet', version: '0.1.0', depField: 'dependencies' }],
      },
    ],
  },
];

const names = (r) => [...r.importers.keys()].sort();

test('pnpm 9 importer shape: dev importers', () => {
  const r = parseWhy(PNPM9_DEV, 'elliptic');
  assert.equal(r.shape, 'importers');
  assert.deepEqual(names(r), ['@botho/snap', '@botho/snap-spike']);
  assert.deepEqual([...r.importers.get('@botho/snap')], ['devDependencies']);
});

test('pnpm 9 importer shape: no prod importers', () => {
  const r = parseWhy(PNPM9_PROD_NONE, 'elliptic');
  assert.equal(r.shape, 'importers');
  assert.deepEqual(names(r), []);
});

test('pnpm 9 importer shape: a prod leak is reported', () => {
  assert.deepEqual(names(parseWhy(PNPM9_PROD_LEAK, 'elliptic')), ['@botho/web-wallet']);
});

test('pnpm >= 10 dependents shape: dev importers, deduped leaves allowed', () => {
  const r = parseWhy(PNPM12_DEV, 'elliptic');
  assert.equal(r.shape, 'dependents');
  assert.deepEqual(names(r), ['@botho/snap', '@botho/snap-spike']);
});

test('pnpm >= 10 dependents shape: a prod leak is reported, including nested importers', () => {
  assert.deepEqual(names(parseWhy(PNPM12_PROD_LEAK, 'elliptic')), ['@botho/ui', '@botho/web-wallet']);
});

test('empty output parses as empty (the caller anchors on @botho/snap)', () => {
  assert.equal(parseWhy('', 'elliptic').shape, 'empty');
  assert.equal(parseWhy([], 'elliptic').importers.size, 0);
});

test('unrecognized shapes throw instead of reporting no importers', () => {
  const bad = [
    { elliptic: { dependents: [] } }, // not an array
    ['elliptic'], // array of non-objects
    [{ name: 'elliptic', version: '6.6.1', dependents: 'x' }], // dependents not an array
    [{ name: 'react', version: '19.0.0', dependents: [] }], // wrong package
    [{ name: 'elliptic', version: '6.6.1', dependents: [{ name: '@x/y', version: '1.0.0' }] }], // bare leaf
    [{ name: '@botho/snap', version: '0.1.0', path: '/w', devDependencies: [] }], // map is not an object
    [{ name: '@botho/snap', version: '0.1.0', path: '/w', importers: {} }], // unknown key
    [{ name: '@botho/snap', version: '0.1.0' }], // importer entry without a path
  ];
  for (const json of bad) {
    assert.throws(() => parseWhy(json, 'elliptic'), UnrecognizedWhyOutput, JSON.stringify(json));
  }
});
