import { describe, it, expect } from 'vitest'
import {
  parseLocalePath,
  buildLocalePath,
  switchLocaleInPath,
} from './locale-path'

describe('parseLocalePath', () => {
  it('treats an unprefixed path as the default (en) locale', () => {
    expect(parseLocalePath('/')).toEqual({ locale: 'en', rest: '/' })
    expect(parseLocalePath('/wallet')).toEqual({ locale: 'en', rest: '/wallet' })
    expect(parseLocalePath('/explorer/tx/abc')).toEqual({
      locale: 'en',
      rest: '/explorer/tx/abc',
    })
  })

  it('extracts a supported non-default locale prefix', () => {
    expect(parseLocalePath('/es')).toEqual({ locale: 'es', rest: '/' })
    expect(parseLocalePath('/es/wallet')).toEqual({ locale: 'es', rest: '/wallet' })
    expect(parseLocalePath('/es/explorer/tx/abc')).toEqual({
      locale: 'es',
      rest: '/explorer/tx/abc',
    })
  })

  it('falls back to en for an unsupported locale prefix (no 404)', () => {
    expect(parseLocalePath('/xx/wallet')).toEqual({
      locale: 'en',
      rest: '/xx/wallet',
    })
  })

  it('does NOT treat an explicit /en prefix as a locale segment', () => {
    // `en` is the unprefixed default; a literal /en path is a normal route.
    expect(parseLocalePath('/en/wallet')).toEqual({
      locale: 'en',
      rest: '/en/wallet',
    })
  })
})

describe('buildLocalePath', () => {
  it('leaves the default locale unprefixed', () => {
    expect(buildLocalePath('en', '/wallet')).toBe('/wallet')
    expect(buildLocalePath('en', '/')).toBe('/')
  })

  it('prefixes non-default locales', () => {
    expect(buildLocalePath('es', '/wallet')).toBe('/es/wallet')
    expect(buildLocalePath('es', '/')).toBe('/es')
  })

  it('normalizes a path missing its leading slash', () => {
    expect(buildLocalePath('es', 'wallet')).toBe('/es/wallet')
  })
})

describe('switchLocaleInPath', () => {
  it('round-trips en -> es -> en preserving the page', () => {
    expect(switchLocaleInPath('/wallet', 'es')).toBe('/es/wallet')
    expect(switchLocaleInPath('/es/wallet', 'en')).toBe('/wallet')
  })

  it('keeps the root page when switching locales', () => {
    expect(switchLocaleInPath('/', 'es')).toBe('/es')
    expect(switchLocaleInPath('/es', 'en')).toBe('/')
  })
})
