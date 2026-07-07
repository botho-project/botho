/**
 * Pure derivations for the explorer's lottery events feed (#699).
 *
 * Displays on-chain facts only — per-block aggregates (payout count/total,
 * pool distributed, fee burn). No payout-recipient linkage tooling, per the
 * privacy posture of the handoff.
 */
import type { Block } from '@botho/core'

/**
 * Select blocks with lottery activity: payoutCount > 0 OR poolDistributed > 0.
 *
 * Blocks from older nodes (no `lottery` field) are excluded rather than
 * rendered as zeros. Returns a NEW array sorted newest first; the input is
 * never mutated.
 */
export function selectLotteryBlocks(blocks: Block[]): Block[] {
  return blocks
    .filter(
      (block) =>
        block.lottery !== undefined &&
        (block.lottery.payoutCount > 0 || block.lottery.poolDistributed > 0n),
    )
    .sort((a, b) => b.height - a.height)
}
