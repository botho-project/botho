import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import {
  AGE_SIMILARITY_SPREAD_BPS,
  MIN_DECOY_AGE_BLOCKS,
  ageSimilarityBand,
  ageSimilarityEnvelope,
  ageSimilarityWindow,
  ageSimilarityWindowWidth,
  drawAgeSimilarityWindow,
  randomIndex,
  randomInteger,
  selectDecoysForInput,
  type ChainOutput,
} from '../src/index'

// Shared with botho/src/decoy_selection.rs
// (`test_age_similarity_band_matches_shared_web_fixture`), so the web and
// native pickers cannot drift apart (#1693).
const fixture = JSON.parse(
  readFileSync(resolve(import.meta.dirname, 'fixtures/decoy-age-band.json'), 'utf8'),
) as {
  minDecoyAgeBlocks: number
  ageSimilaritySpreadBps: number
  bands: Array<{ realAge: number; minAge: number; maxAge: number; degenerate: boolean }>
  windows: Array<{
    realAge: number
    offset: number
    width: number
    minAge: number
    maxAge: number
    degenerate: boolean
  }>
  fallback: Array<{
    name: string
    realAge: number
    count: number
    poolAges: number[]
    expectedAges: number[]
  }>
}

const TIP = 100_000

function row(id: number, age: number): ChainOutput {
  return {
    targetKey: id.toString(16).padStart(64, '0'),
    publicKey: (id + 1_000_000).toString(16).padStart(64, '0'),
    amount: 1n,
    height: TIP - age,
  }
}
const ageOf = (o: ChainOutput) => TIP - (o.height as number)

describe('ageSimilarityBand matches the native age_similarity_band (shared fixture)', () => {
  it('pins the shared constants', () => {
    expect(fixture.minDecoyAgeBlocks).toBe(MIN_DECOY_AGE_BLOCKS)
    expect(fixture.ageSimilaritySpreadBps).toBe(AGE_SIMILARITY_SPREAD_BPS)
  })

  it.each(fixture.bands)('band for real age $realAge', ({ realAge, minAge, maxAge, degenerate }) => {
    const [lo, hi] = ageSimilarityBand(realAge)
    expect([lo, hi]).toEqual([minAge, maxAge])
    expect(lo > hi).toBe(degenerate)
  })

  it.each(fixture.windows)(
    'random-offset window for real age $realAge at offset $offset',
    ({ realAge, offset, width, minAge, maxAge, degenerate }) => {
      expect(ageSimilarityWindowWidth(realAge)).toBe(width)
      const [lo, hi] = ageSimilarityWindow(realAge, offset)
      expect([lo, hi]).toEqual([minAge, maxAge])
      expect(lo > hi).toBe(degenerate)
    },
  )

  it.each(fixture.fallback)('fallback: $name', ({ realAge, count, poolAges, expectedAges }) => {
    const pool = poolAges.map((a) => row(a, a))
    for (let draw = 0; draw < 32; draw++) {
      const picked = selectDecoysForInput(pool, count, realAge, TIP).map(ageOf)
      expect([...picked].sort((a, b) => a - b)).toEqual([...expectedAges].sort((a, b) => a - b))
    }
  })
})

describe('selectDecoysForInput', () => {
  // A wide pool: one output per age 0..2999, in chain (age-descending) order.
  const pool = Array.from({ length: 3000 }, (_, i) => row(i + 1, 2999 - i))

  it('keeps every ring inside one random-offset window across draws and real ages', () => {
    // Real ages whose window holds >= 19 candidates at every offset (younger
    // ones hit the nearest-age fallback, pinned by the shared fixture above).
    for (const realAge of [100, 777, 1500, 2400]) {
      const [lo, hi] = ageSimilarityEnvelope(realAge)
      const width = ageSimilarityWindowWidth(realAge)
      for (let draw = 0; draw < 200; draw++) {
        const ring = selectDecoysForInput(pool, 19, realAge, TIP)
        expect(ring).toHaveLength(19)
        expect(new Set(ring.map((o) => o.targetKey)).size).toBe(19)
        const ages = ring.map(ageOf)
        for (const age of ages) {
          expect(age).toBeGreaterThanOrEqual(lo)
          expect(age).toBeLessThanOrEqual(hi)
          expect(age).toBeGreaterThanOrEqual(MIN_DECOY_AGE_BLOCKS)
        }
        // The ring and the real input fit in a single window.
        const all = [...ages, realAge]
        expect(Math.max(...all) - Math.min(...all)).toBeLessThanOrEqual(width)
      }
    }
  })

  it('places the real input at a uniform age rank (#1692 / #1755)', () => {
    // One output per age: the real input's rank is the number of younger
    // decoys (no ties with the real age: the pool's output at the real age is
    // the real input itself and is excluded).
    const realAge = 1500
    const ringPool = pool.filter((o) => ageOf(o) !== realAge)
    const rings = 4000
    const hist = new Array(20).fill(0)
    for (let i = 0; i < rings; i++) {
      const ages = selectDecoysForInput(ringPool, 19, realAge, TIP).map(ageOf)
      hist[ages.filter((a) => a < realAge).length]++
    }
    const expected = rings / 20
    const chi2 = hist.reduce((s, o) => s + (o - expected) ** 2 / expected, 0)
    // df 19, p = 0.01 critical value. A centred band gives χ² in the thousands.
    expect(chi2).toBeLessThan(36.191)
  })

  it('ring membership differs across repeated draws for the same input', () => {
    const rings = Array.from({ length: 20 }, () =>
      selectDecoysForInput(pool, 19, 1000, TIP)
        .map((o) => o.targetKey)
        .sort()
        .join(','),
    )
    expect(new Set(rings).size).toBeGreaterThan(1)
  })

  it('does not pick the chain-ordered oldest window any more', () => {
    const ring = selectDecoysForInput(pool, 19, 100, TIP).map(ageOf)
    // The pre-#1693 picker returned pool[0..19] = the 19 OLDEST outputs.
    expect(Math.max(...ring)).toBeLessThanOrEqual(120)
  })

  it('excludes unconfirmed outputs and fails with fewer than count confirmed', () => {
    const young = Array.from({ length: 30 }, (_, i) => row(i + 1, i % MIN_DECOY_AGE_BLOCKS))
    expect(() => selectDecoysForInput(young, 19, 5, TIP)).toThrow(/Not enough decoys on chain/)
  })

  it('fails closed when an output has no height', () => {
    const noHeight = pool.slice(0, 40).map(({ height: _h, ...rest }) => rest as ChainOutput)
    expect(() => selectDecoysForInput(noHeight, 19, 100, TIP)).toThrow(/no block height/)
  })
})

describe('drawAgeSimilarityWindow', () => {
  it('always contains the real age and spans the window width', () => {
    for (const realAge of [10, 11, 50, 1000, 1_000_000]) {
      const width = ageSimilarityWindowWidth(realAge)
      const [envLo, envHi] = ageSimilarityEnvelope(realAge)
      for (let i = 0; i < 200; i++) {
        const [lo, hi] = drawAgeSimilarityWindow(realAge)
        expect(lo).toBeLessThanOrEqual(realAge)
        expect(hi).toBeGreaterThanOrEqual(realAge)
        expect(lo).toBeGreaterThanOrEqual(envLo)
        expect(hi).toBeLessThanOrEqual(envHi)
        expect(hi - lo).toBeLessThanOrEqual(width)
      }
    }
    expect(ageSimilarityWindow(1000, 100)).toEqual(ageSimilarityBand(1000))
  })
})

describe('randomInteger', () => {
  it('delegates small bounds to randomIndex and rejects biased 53-bit draws', () => {
    const small = [7]
    expect(randomInteger(3, (a) => ((a[0] = small.shift() as number), a))).toBe(1)
    // n = 2^32 + 1: the 53-bit draw (w0 >>> 11) * 2^32 + w1 is accepted
    // below 2^53 - (2^53 % n).
    const words = [0, 5]
    const fill = (a: Uint32Array) => {
      for (let i = 0; i < a.length; i++) a[i] = words.shift() as number
      return a
    }
    expect(randomInteger(2 ** 32 + 1, fill)).toBe(5)
    expect(() => randomInteger(0)).toThrow(/invalid bound/)
  })
})

describe('randomIndex', () => {
  it('rejects biased draws and stays in range', () => {
    // n = 3: limit = 2^32 - (2^32 % 3) = 4294967295; the value 4294967295 is
    // rejected, then 7 maps to 7 % 3 = 1.
    const seq = [0xffffffff, 7]
    const fill = (a: Uint32Array) => {
      a[0] = seq.shift() as number
      return a
    }
    expect(randomIndex(3, fill)).toBe(1)
    for (let i = 0; i < 1000; i++) {
      const v = randomIndex(17)
      expect(v).toBeGreaterThanOrEqual(0)
      expect(v).toBeLessThan(17)
    }
  })

  it('throws instead of degrading when no CSPRNG exists', () => {
    const original = globalThis.crypto
    Object.defineProperty(globalThis, 'crypto', { value: undefined, configurable: true })
    try {
      expect(() => randomIndex(5)).toThrow(/CSPRNG/)
    } finally {
      Object.defineProperty(globalThis, 'crypto', { value: original, configurable: true })
    }
  })
})
