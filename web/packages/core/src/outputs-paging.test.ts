import { describe, it, expect } from 'vitest'
import { MAX_GET_OUTPUTS_BLOCKS, fetchOutputBlocksPaged } from './outputs-paging'

/** A node with blocks `0..=tip` serving at most `cap` per call (#1656). */
function cappedNode(tip: number, cap: number) {
  const calls: Array<[number, number]> = []
  const fetch = (end: number) => async (from: number) => {
    calls.push([from, end])
    const last = Math.min(end, from + cap - 1, tip)
    const out: Array<{ height: number; outputs: string[] }> = []
    for (let h = from; h <= last; h++) out.push({ height: h, outputs: [`out-${h}`] })
    return out
  }
  return { calls, fetch }
}

describe('fetchOutputBlocksPaged (#1656)', () => {
  it('pages a [0, tip] fetch across the server cap without missing blocks', async () => {
    const tip = 2_345
    const node = cappedNode(tip, MAX_GET_OUTPUTS_BLOCKS)
    const blocks = await fetchOutputBlocksPaged(0, tip, node.fetch(tip))
    expect(blocks.map((b) => b.height)).toEqual(Array.from({ length: tip + 1 }, (_, i) => i))
    expect(blocks.flatMap((b) => b.outputs)).toHaveLength(tip + 1)
    expect(node.calls).toEqual([[0, tip], [1000, tip], [2000, tip]])
  })

  it('stops at the first empty page past the tip', async () => {
    const node = cappedNode(10, 4)
    const blocks = await fetchOutputBlocksPaged(5, 1_000_000, node.fetch(1_000_000))
    expect(blocks.map((b) => b.height)).toEqual([5, 6, 7, 8, 9, 10])
    expect(node.calls.map(([from]) => from)).toEqual([5, 9, 11])
  })

  it('makes one call when the range fits a page and none when it is empty', async () => {
    const node = cappedNode(100, MAX_GET_OUTPUTS_BLOCKS)
    expect(await fetchOutputBlocksPaged(3, 7, node.fetch(7))).toHaveLength(5)
    expect(await fetchOutputBlocksPaged(9, 3, node.fetch(3))).toEqual([])
    expect(node.calls).toEqual([[3, 7]])
  })

  it('takes a height-less page (legacy fixture shape) as complete', async () => {
    let calls = 0
    const blocks = await fetchOutputBlocksPaged(0, 5_000, async () => {
      calls++
      return [{ outputs: ['a'] }, { outputs: ['b'] }] as Array<{ height?: number; outputs: string[] }>
    })
    expect(blocks).toHaveLength(2)
    expect(calls).toBe(1)
  })

  it('rejects a page that does not advance instead of looping', async () => {
    await expect(fetchOutputBlocksPaged(10, 20, async () => [{ height: 3 }])).rejects.toThrow(/no progress/)
  })
})
