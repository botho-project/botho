import { describe, it, expect } from 'vitest'
import { formatRelativeTime, formatAbsoluteTime } from './format'

describe('formatRelativeTime', () => {
  // Fixed "now" so the relative output is deterministic.
  const nowMs = 1_700_000_000_000
  const nowSec = Math.floor(nowMs / 1000)

  const at = (secondsAgo: number) => formatRelativeTime(nowSec - secondsAgo, nowMs)

  it('shows "just now" for very recent timestamps', () => {
    expect(at(0)).toBe('just now')
    expect(at(10)).toBe('just now')
    expect(at(44)).toBe('just now')
  })

  it('treats future timestamps (clock skew) as "just now"', () => {
    expect(formatRelativeTime(nowSec + 30, nowMs)).toBe('just now')
  })

  it('shows minutes', () => {
    expect(at(60)).toBe('1m ago')
    expect(at(120)).toBe('2m ago')
    expect(at(59 * 60)).toBe('59m ago')
  })

  it('shows hours', () => {
    expect(at(60 * 60)).toBe('1h ago')
    expect(at(3 * 60 * 60)).toBe('3h ago')
  })

  it('shows "yesterday" for ~1 day ago', () => {
    expect(at(24 * 60 * 60 + 60)).toBe('yesterday')
  })

  it('shows days for the rest of the week', () => {
    expect(at(3 * 24 * 60 * 60)).toBe('3d ago')
  })

  it('falls back to an absolute date for older than a week', () => {
    const old = nowSec - 30 * 24 * 60 * 60
    expect(formatRelativeTime(old, nowMs)).toBe(
      new Date(old * 1000).toLocaleDateString()
    )
  })
})

describe('formatAbsoluteTime', () => {
  const ts = 1_700_000_000

  it('renders a full date/time by default', () => {
    expect(formatAbsoluteTime(ts)).toBe(new Date(ts * 1000).toLocaleString())
  })

  it('renders date only when requested', () => {
    expect(formatAbsoluteTime(ts, { dateOnly: true })).toBe(
      new Date(ts * 1000).toLocaleDateString()
    )
  })
})
