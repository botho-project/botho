/**
 * Build-time Subresource Integrity (SRI) contract for the operator dashboard
 * entry (#772, §8.3.1 option (a)).
 *
 * The operator dashboard is split into its own Vite build target (`operator.html`
 * -> `operator-main.tsx`) so its emitted HTML document references only its own
 * hashed chunks and pins `integrity="sha384-…"` on each `<script>`/`<link>` tag
 * (`sriHashPlugin` in `vite.config.ts`). Browser-enforced SRI is the entire
 * point of the split: a tampered chunk on the host fails to load with no
 * operator action required.
 *
 * This test runs a real production build in-process (fast — a few seconds) into
 * a temp dir, then asserts, byte-for-byte against the files on disk, that:
 *   1. every local (root-relative) <script src>/<link rel=stylesheet|modulepreload href>
 *      reference in `operator.html` carries an `integrity="sha384-…"` attribute, and
 *   2. each pinned hash equals the sha384 of the referenced emitted asset.
 *
 * If the SRI plugin regresses (missing attribute, stale/incorrect hash), this
 * test fails, which fails `pnpm test:run` / CI.
 */
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { build } from 'vite'
import { createHash } from 'node:crypto'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const PKG_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

const sri = (bytes: Buffer): string =>
  'sha384-' + createHash('sha384').update(bytes).digest('base64')

/** Extract same-origin, root-relative script/style/modulepreload refs + any integrity. */
function collectAssetTags(
  html: string,
): { attr: string; ref: string; integrity: string | null }[] {
  const out: { attr: string; ref: string; integrity: string | null }[] = []
  const tagRe = /<(script|link)\b[^>]*>/gi
  let match: RegExpExecArray | null
  while ((match = tagRe.exec(html)) !== null) {
    const tag = match[0]
    const isScript = /^<script/i.test(tag)
    const isStyle = /^<link/i.test(tag) && /rel\s*=\s*["']stylesheet["']/i.test(tag)
    const isPreload =
      /^<link/i.test(tag) && /rel\s*=\s*["']modulepreload["']/i.test(tag)
    if (!isScript && !isStyle && !isPreload) continue
    const attr = isScript ? 'src' : 'href'
    const refM = tag.match(new RegExp(`\\b${attr}\\s*=\\s*["']([^"']+)["']`, 'i'))
    if (!refM) continue
    const ref = refM[1]
    // Only local (root-relative) asset chunks are integrity-checkable here.
    if (!ref.startsWith('/assets/')) continue
    const intM = tag.match(/\bintegrity\s*=\s*["']([^"']+)["']/i)
    out.push({ attr, ref, integrity: intM ? intM[1] : null })
  }
  return out
}

describe('operator entry Subresource Integrity', () => {
  let dist: string

  beforeAll(async () => {
    dist = mkdtempSync(path.join(tmpdir(), 'operator-sri-'))
    await build({
      root: PKG_ROOT,
      logLevel: 'silent',
      build: {
        outDir: dist,
        emptyOutDir: true,
        // Source maps are irrelevant to the SRI contract and slow the build.
        sourcemap: false,
      },
    })
  }, 120_000)

  afterAll(() => {
    if (dist) rmSync(dist, { recursive: true, force: true })
  })

  it('pins a matching sha384 integrity hash on every operator asset reference', () => {
    const html = readFileSync(path.join(dist, 'operator.html'), 'utf8')
    const tags = collectAssetTags(html)

    // Sanity: the operator entry must reference at least its own JS entry chunk
    // and its CSS — a zero-reference document would trivially "pass".
    expect(tags.length).toBeGreaterThanOrEqual(2)

    for (const { attr, ref, integrity } of tags) {
      expect(
        integrity,
        `operator.html ${attr}="${ref}" is missing an integrity attribute`,
      ).not.toBeNull()

      const onDisk = readFileSync(path.join(dist, ref.replace(/^\//, '')))
      expect(
        integrity,
        `operator.html ${attr}="${ref}" integrity does not match the file on disk`,
      ).toBe(sri(onDisk))
    }
  })

  it('does not pin integrity on the main SPA entry (index.html stays SW-managed)', () => {
    // The main SPA is under PWA/auto-update service-worker control; pinning SRI
    // on a document the SW may replace would be self-defeating (§8.3.1). Assert
    // the split is targeted: index.html carries no integrity attributes.
    const html = readFileSync(path.join(dist, 'index.html'), 'utf8')
    expect(html).not.toMatch(/\bintegrity\s*=/i)
  })
})
