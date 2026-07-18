/**
 * Key-derivation determinism + address validity (issue #815 deliverable 2),
 * exercised through the OFFICIAL `@metamask/snaps-jest` harness — the snap
 * bundle (with the `bth-wasm-signer` wasm inlined) runs inside the real MetaMask
 * Snaps SES executor via `@metamask/snaps-simulation`.
 */

import { describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';

/** Unwrap a snaps-jest response, failing loudly on a snap-side error. */
function unwrap<T>(response: { response: unknown }): T {
  const res = response.response as { result?: T; error?: { message?: string } };
  if (res.error) {
    throw new Error(`snap returned error: ${JSON.stringify(res.error)}`);
  }
  return res.result as T;
}

describe('botho snap: MetaMask-SRP -> Botho-key derivation', () => {
  it('derives a valid testnet v2 address from MetaMask entropy (SIP-6)', async () => {
    const { request } = await installSnap();
    const { address, derivation } = unwrap<{ address: string; derivation: string }>(
      await request({ method: 'botho_getAddress' }),
    );

    // A structurally valid v2 testnet address that carries the ML-KEM + ML-DSA
    // keys (so it is a real post-quantum receive address, not a v1 stub).
    expect(address.startsWith('tbotho://2/')).toBe(true);
    expect(address.length).toBeGreaterThan(1000);
    // The documented derivation path is surfaced to the caller.
    expect(derivation).toContain("m/44'/866'/0'");
    expect(derivation).toContain('snap_getEntropy');
  });

  it('is deterministic: a re-install under the same SRP derives the same wallet', async () => {
    const first = await installSnap();
    const a = unwrap<{ address: string }>(await first.request({ method: 'botho_getAddress' }));

    const second = await installSnap();
    const b = unwrap<{ address: string }>(await second.request({ method: 'botho_getAddress' }));

    expect(b.address).toBe(a.address);
  });
});
