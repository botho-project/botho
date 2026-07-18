/**
 * Pure-logic unit tests (no SES install needed): the wrong-network guard and
 * SES-safe amount formatting. These run under the snaps-jest preset but exercise
 * the modules directly — no `installSnap`, no wasm — so the loopback-exemption
 * branch of the guard (which the loopback mock node can't trigger) is covered.
 */

import { describe, expect, it } from '@jest/globals';

import { assertNetworkAllowed, isValidRpcUrl, EXPECTED_NETWORK_ID } from '../src/node';
import { formatPicocreditsBTH, formatBTHWithUnit } from '../src/format';

describe('node ingress guard', () => {
  it('accepts https and loopback-http endpoints, rejects the rest', () => {
    expect(isValidRpcUrl('https://seed.botho.io/rpc')).toBe(true);
    expect(isValidRpcUrl('http://localhost:17101/rpc')).toBe(true);
    expect(isValidRpcUrl('http://127.0.0.1:8545')).toBe(true);
    expect(isValidRpcUrl('http://seed.botho.io/rpc')).toBe(false); // plain-http remote
    expect(isValidRpcUrl('ftp://seed.botho.io')).toBe(false);
    expect(isValidRpcUrl('not-a-url')).toBe(false);
  });

  it('rejects a remote node on a different network (#811 wrong-network guard)', () => {
    expect(() =>
      assertNetworkAllowed('https://seed.botho.io/rpc', 'botho-mainnet'),
    ).toThrow(/different network/i);
  });

  it('accepts a remote node on the expected network', () => {
    expect(() =>
      assertNetworkAllowed('https://seed.botho.io/rpc', EXPECTED_NETWORK_ID),
    ).not.toThrow();
  });

  it('exempts loopback hosts from the network match (local dev)', () => {
    expect(() =>
      assertNetworkAllowed('http://127.0.0.1:8545', 'botho-devnet'),
    ).not.toThrow();
    expect(() =>
      assertNetworkAllowed('http://localhost:17101/rpc', 'anything'),
    ).not.toThrow();
  });
});

describe('SES-safe BTH formatting', () => {
  it('formats picocredits as trimmed fixed-point BTH', () => {
    expect(formatPicocreditsBTH(0n)).toBe('0');
    expect(formatPicocreditsBTH(5_000_000_000_000n)).toBe('5'); // 5 BTH
    expect(formatPicocreditsBTH(1_234_500_000_000n)).toBe('1.2345');
    expect(formatPicocreditsBTH(100_000_000n)).toBe('0.0001'); // min fee
  });

  it('appends the BTH unit', () => {
    expect(formatBTHWithUnit(1_000_000_000_000n)).toBe('1 BTH');
  });
});
