/**
 * Fee consent in the Snap (#2001, Judge review on PR #2019). PURE-LOGIC tests
 * (no `installSnap`), the same pattern as `units.snap.ts`.
 *
 * Since protocol 10.0.0 the node prices every send, and its floor can be far
 * above the fee the MetaMask dialog showed. `botho_send` and `botho_claimLink`
 * build through `buildWithFeeConsent` with `maxFee` = the confirmed fee, so a
 * higher quote stops BEFORE signing and the Snap re-asks with
 * `feeRaisedContent`. These tests drive the same consent loop the handlers use
 * (a scripted node quote, a scripted dialog) and the dialog it renders. The
 * signing loop itself is covered against a fake signer in
 * `wasm-signer/test/send-fee-quote.test.ts`.
 */

import { describe, expect, it } from '@jest/globals';
import {
  FeeAboveApprovedError,
  buildWithFeeConsent,
  isFeeAboveApprovedError,
} from '@botho/wasm-signer';

import { feeRaisedContent } from '../src/ui';
import { t } from '../src/i18n';

const MIN_FEE = 100_000_000n;
const REQUIRED = 2_062_500_076n; // resignFee for a ~2e9 epoch-0 exit floor

/**
 * Stand-in for `buildSendTransaction` with `fee = maxFee = approved`: it
 * throws the typed error when the node's required fee exceeds the approved
 * one, and records every fee it would have signed at.
 */
function scriptedBuild(required: bigint) {
  const signedAt: bigint[] = [];
  const build = async (approved: bigint) => {
    signedAt.push(approved);
    if (required > approved) throw new FeeAboveApprovedError(required, approved);
    return { txHex: `fee-${approved}` };
  };
  return { build, signedAt };
}

describe('Snap fee consent (#2001)', () => {
  it('re-asks with the required fee and builds only at the confirmed fee', async () => {
    const { build, signedAt } = scriptedBuild(REQUIRED);
    const dialogs: string[] = [];
    const { result, fee } = await buildWithFeeConsent({
      approvedFee: MIN_FEE,
      build,
      reconfirm: async (required, approved) => {
        dialogs.push(
          JSON.stringify(
            feeRaisedContent(
              {
                approvedFeePicocredits: approved,
                requiredFeePicocredits: required,
                amountPicocredits: 1_000_000_000_000n,
                recipientAddress: 'tbotho://2/recipient',
                rpcUrl: 'https://seed.botho.io/rpc',
              },
              'en',
            ),
          ),
        );
        return true;
      },
      onReject: () => new Error('rejected'),
    });
    expect(fee).toBe(REQUIRED);
    expect(result.txHex).toBe(`fee-${REQUIRED}`);
    expect(signedAt).toEqual([MIN_FEE, REQUIRED]);
    expect(dialogs).toHaveLength(1);
    expect(dialogs[0]).toContain('Higher network fee');
    expect(dialogs[0]).toContain('Required fee');
    expect(dialogs[0]).toContain('tbotho://2/recipient');
  });

  it('a declined re-confirmation rejects and never builds above the approved fee', async () => {
    const { build, signedAt } = scriptedBuild(REQUIRED);
    await expect(
      buildWithFeeConsent({
        approvedFee: MIN_FEE,
        build,
        reconfirm: async () => false,
        onReject: () => new Error(t('error.rejectFee', 'en')),
      }),
    ).rejects.toThrow(/rejected the higher network fee/u);
    expect(signedAt).toEqual([MIN_FEE]);
  });

  it('a hostile quote (fee ~ the whole balance) is refused when the user declines', async () => {
    const balance = 1_000_000_000_000n;
    const { build, signedAt } = scriptedBuild(balance - 1n);
    let asked: bigint | undefined;
    const err = await buildWithFeeConsent({
      approvedFee: MIN_FEE,
      build,
      reconfirm: async (required) => {
        asked = required;
        return false;
      },
      onReject: () => new Error('rejected'),
    }).catch((e: unknown) => e);
    expect((err as Error).message).toBe('rejected');
    expect(asked).toBe(balance - 1n);
    expect(signedAt).toEqual([MIN_FEE]);
  });

  it('stops re-asking after the re-confirm limit and surfaces the typed error', async () => {
    // A node that raises its floor after every approval.
    let required = REQUIRED;
    const build = async (approved: bigint) => {
      if (required > approved) throw new FeeAboveApprovedError(required, approved);
      return approved;
    };
    const err = await buildWithFeeConsent({
      approvedFee: MIN_FEE,
      build,
      reconfirm: async () => {
        required *= 2n;
        return true;
      },
      onReject: () => new Error('rejected'),
      maxReconfirms: 2,
    }).catch((e: unknown) => e);
    expect(isFeeAboveApprovedError(err)).toBe(true);
  });

  it('renders the dialog in every locale (no missing keys)', () => {
    for (const locale of ['en', 'es', 'zh'] as const) {
      const rendered = JSON.stringify(
        feeRaisedContent(
          {
            approvedFeePicocredits: MIN_FEE,
            requiredFeePicocredits: REQUIRED,
            amountPicocredits: 5n,
            rpcUrl: 'https://seed.botho.io/rpc',
          },
          locale,
        ),
      );
      expect(rendered).toContain(t('fee.required', locale));
      expect(rendered).not.toContain('fee.required');
    }
  });
});
