/**
 * Paging over the node's `chain_getOutputs` RPC (#1656).
 *
 * The node serves at most {@link MAX_GET_OUTPUTS_BLOCKS} blocks per call and
 * CLAMPS a longer `[start_height, end_height]` request instead of rejecting
 * it. The result is a dense run of block entries from `start_height` (an
 * empty block still has an entry), so a caller that wants the whole range
 * resumes at the last served `height + 1` until it reaches `end` or a page
 * comes back empty (past the tip). A wallet that took one clamped page as the
 * whole range would silently stop scanning at the cap and miss later funds,
 * so every whole-range fetch (`getOutputs(0, tip)`) must go through
 * {@link fetchOutputBlocksPaged}.
 */

/** The node's per-call `chain_getOutputs` block cap (`botho::rpc::MAX_GET_OUTPUTS_BLOCKS`). */
export const MAX_GET_OUTPUTS_BLOCKS = 1000

/**
 * Collect every `chain_getOutputs` block entry in `[start, end]`.
 *
 * `fetchPage(from)` must request `[from, end]` from the node. Against an
 * uncapped node the first page already covers the range (plus one empty
 * probe when the range runs past the tip). A page that does not advance is an
 * error rather than an infinite loop.
 */
export async function fetchOutputBlocksPaged<B extends { height?: number }>(
  start: number,
  end: number,
  fetchPage: (from: number) => Promise<B[]>,
): Promise<B[]> {
  const blocks: B[] = []
  let next = start
  while (next <= end) {
    const page = await fetchPage(next)
    if (page.length === 0) break
    const last = page[page.length - 1].height
    for (const block of page) blocks.push(block)
    // The node always sends `height`; a height-less page (hand-written test
    // fixtures, pre-height mocks) cannot be paged and is taken as complete.
    if (typeof last !== 'number') break
    if (last < next) {
      throw new Error(`chain_getOutputs page from ${next} ended at ${last}: no progress`)
    }
    if (last >= end) break
    next = last + 1
  }
  return blocks
}
