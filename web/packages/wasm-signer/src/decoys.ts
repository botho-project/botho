/**
 * Client-side ring-decoy selection, aligned with the native wallet (#1693).
 *
 * Before #1693 the web/Snap/claim-link send path filled every ring from the
 * SAME chain-ordered window of outputs (`pool[(i*k + j) % len]`): no
 * randomness, no age band and no confirmation floor. A ring whose real input
 * was young therefore carried 19 of the chain's oldest outputs, so the real
 * input was the odd one out.
 *
 * This module ports the native policy from `botho/src/decoy_selection.rs`:
 *
 * - candidates younger than {@link MIN_DECOY_AGE_BLOCKS} are never decoys;
 * - each decoy is drawn from the {@link ageSimilarityBand} around the real
 *   input's age (±{@link AGE_SIMILARITY_SPREAD_BPS} bps, lower bound floored at
 *   the confirmation minimum), identical integer bps math to the Rust
 *   `age_similarity_band` — a shared fixture
 *   (`test/fixtures/decoy-age-band.json`) is checked by BOTH the vitest suite
 *   and a Rust unit test so the two cannot drift;
 * - when the band cannot supply enough members (sparse chain or a degenerate
 *   band) the remaining slots are filled with the nearest-aged eligible
 *   candidates, exactly as `GammaDecoySelector::select_decoys_for_input` does;
 * - in-band sampling is uniform without replacement driven by a CSPRNG
 *   (`crypto.getRandomValues` with rejection sampling — never `Math.random`).
 *
 * Gamma/OSPEAD weighting inside the band is a native refinement not ported
 * here; band + uniform CSPRNG is the required minimum (Curator plan, #1693).
 *
 * This is WALLET POLICY only. The node validates ring well-formedness, not how
 * members were chosen, so nothing here changes consensus or ring size.
 */

import type { ChainOutput } from './index'

/**
 * Minimum decoy age in blocks (confirmation floor). Mirrors
 * `MIN_DECOY_AGE_BLOCKS` in `botho/src/decoy_selection.rs`.
 */
export const MIN_DECOY_AGE_BLOCKS = 10

/**
 * Age-similarity spread in basis points (1_000 = ±10%). Mirrors
 * `AGE_SIMILARITY_SPREAD_BPS` in `botho/src/decoy_selection.rs`.
 */
export const AGE_SIMILARITY_SPREAD_BPS = 1_000

/**
 * Inclusive `[minAge, maxAge]` decoy-age band around a real input's age. A
 * byte-for-byte port of the Rust `age_similarity_band` (u128 bps product,
 * floor division, saturating subtraction, lower bound floored at
 * {@link MIN_DECOY_AGE_BLOCKS}). A real age below the floor yields a
 * degenerate band (`minAge > maxAge`): nothing is in-band and selection falls
 * back to nearest-age fill.
 */
export function ageSimilarityBand(realInputAge: number): [number, number] {
  if (!Number.isSafeInteger(realInputAge) || realInputAge < 0) {
    throw new Error(`Invalid real input age: ${realInputAge}`)
  }
  const age = BigInt(realInputAge)
  const delta = (age * BigInt(AGE_SIMILARITY_SPREAD_BPS)) / 10_000n
  let min = age > delta ? age - delta : 0n
  if (min < BigInt(MIN_DECOY_AGE_BLOCKS)) min = BigInt(MIN_DECOY_AGE_BLOCKS)
  const max = age + delta
  return [Number(min), Number(max)]
}

/** Source of cryptographically secure random bytes. */
export type RandomFill = (bytes: Uint32Array) => Uint32Array

function defaultRandomFill(bytes: Uint32Array): Uint32Array {
  const c = (globalThis as { crypto?: { getRandomValues?: (a: Uint32Array) => Uint32Array } })
    .crypto
  if (!c || typeof c.getRandomValues !== 'function') {
    // Fail closed: never degrade to a predictable source for ring privacy.
    throw new Error('No CSPRNG available (crypto.getRandomValues) for decoy selection')
  }
  return c.getRandomValues(bytes)
}

/**
 * Uniform integer in `[0, n)` from a CSPRNG via rejection sampling, so there is
 * no modulo bias. `n` must be in `[1, 2^32]`.
 */
export function randomIndex(n: number, fill: RandomFill = defaultRandomFill): number {
  if (!Number.isInteger(n) || n < 1 || n > 0x1_0000_0000) {
    throw new Error(`randomIndex: invalid bound ${n}`)
  }
  if (n === 1) return 0
  const range = 0x1_0000_0000
  const limit = range - (range % n)
  const buf = new Uint32Array(1)
  for (;;) {
    fill(buf)
    const v = buf[0]
    if (v < limit) return v % n
  }
}

/** In-place Fisher-Yates shuffle driven by {@link randomIndex}. */
function shuffle<T>(items: T[], fill: RandomFill): T[] {
  for (let i = items.length - 1; i > 0; i--) {
    const j = randomIndex(i + 1, fill)
    const tmp = items[i]
    items[i] = items[j]
    items[j] = tmp
  }
  return items
}

/** A decoy candidate with its age resolved against the chain tip. */
interface AgedCandidate {
  output: ChainOutput
  age: number
}

/**
 * Resolve a row's age in blocks at `chainHeight`. Throws when the height is
 * missing: picking decoys without ages would silently regress to
 * chain-ordered rings, so the send fails closed instead (#1693).
 */
export function outputAge(output: Pick<ChainOutput, 'height' | 'targetKey'>, chainHeight: number): number {
  const h = output.height
  if (typeof h !== 'number' || !Number.isSafeInteger(h) || h < 0) {
    throw new Error(
      'Cannot select ring decoys: chain output ' +
        `${output.targetKey.slice(0, 16)}… has no block height. The node adapter ` +
        'must pass each output\'s block height through from chain_getOutputs ' +
        '(refusing to fall back to chain-ordered decoys, #1693).',
    )
  }
  return Math.max(0, chainHeight - h)
}

/**
 * Select `count` decoys for one real input of age `realInputAge`.
 *
 * `pool` must already exclude every real input, lottery rows and placeholder
 * keys; this function applies the confirmation floor, the age band and the
 * nearest-age fallback. Throws `Not enough decoys on chain …` when fewer than
 * `count` eligible (confirmed) candidates exist.
 */
export function selectDecoysForInput(
  pool: ChainOutput[],
  count: number,
  realInputAge: number,
  chainHeight: number,
  fill: RandomFill = defaultRandomFill,
): ChainOutput[] {
  const eligible: AgedCandidate[] = []
  for (const output of pool) {
    const age = outputAge(output, chainHeight)
    if (age >= MIN_DECOY_AGE_BLOCKS) eligible.push({ output, age })
  }
  if (eligible.length < count) {
    throw new Error(
      `Not enough decoys on chain for a ring of ${count + 1}: ` +
        `need ${count} confirmed (>= ${MIN_DECOY_AGE_BLOCKS} blocks old) per input, ` +
        `found ${eligible.length}. Mine more blocks / wait for more on-chain outputs.`,
    )
  }

  const [minAge, maxAge] = ageSimilarityBand(realInputAge)
  const inBand: AgedCandidate[] = []
  const outOfBand: AgedCandidate[] = []
  for (const c of eligible) {
    if (minAge <= maxAge && c.age >= minAge && c.age <= maxAge) inBand.push(c)
    else outOfBand.push(c)
  }

  // Phase 1: uniform sample WITHOUT replacement from the band (partial
  // Fisher-Yates over a copy, CSPRNG-driven).
  const selected: AgedCandidate[] = []
  const take = Math.min(count, inBand.length)
  for (let i = 0; i < take; i++) {
    const j = i + randomIndex(inBand.length - i, fill)
    const tmp = inBand[i]
    inBand[i] = inBand[j]
    inBand[j] = tmp
    selected.push(inBand[i])
  }

  // Phase 2 (liveness fallback, mirrors the native selector): fill remaining
  // slots with the nearest-aged eligible candidates. Shuffle first so equal
  // distances break ties randomly rather than by chain order (stable sort).
  if (selected.length < count) {
    const leftovers = shuffle([...outOfBand], fill).sort(
      (a, b) => Math.abs(a.age - realInputAge) - Math.abs(b.age - realInputAge),
    )
    for (const c of leftovers) {
      if (selected.length >= count) break
      selected.push(c)
    }
  }

  // Do not leak selection order (in-band first, fallback last) into the ring.
  return shuffle(selected, fill).map((c) => c.output)
}
