import { describe, it, expect, beforeEach } from 'vitest'
import {
  ClaimLinkStore,
  LocalStorageClaimLinks,
  CLAIM_LINK_EXPIRY_WINDOW_SECONDS,
  type ClaimLinkRecord,
} from './claim-links'

const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => { store[key] = value },
    removeItem: (key: string) => { delete store[key] },
    clear: () => { store = {} },
  }
})()

Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

const SAMPLE = {
  ephMnemonic:
    'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
  ephAddress: 'tbotho://1/sample',
  amount: 5_000_000_000_000n,
}

describe('ClaimLinkStore', () => {
  beforeEach(() => {
    localStorageMock.clear()
  })

  it('adds and lists records', async () => {
    const store = new ClaimLinkStore()
    await store.load()
    const rec = await store.add(SAMPLE)
    expect(rec.status).toBe('outstanding')
    expect(rec.id).toBeTruthy()
    expect(store.getAll()).toHaveLength(1)
    expect(store.getAll()[0].amount).toBe(SAMPLE.amount)
  })

  it('persists bigint amounts across reload (round-trip serialization)', async () => {
    const store1 = new ClaimLinkStore()
    await store1.load()
    await store1.add(SAMPLE)

    const store2 = new ClaimLinkStore()
    await store2.load()
    const all = store2.getAll()
    expect(all).toHaveLength(1)
    expect(all[0].amount).toBe(SAMPLE.amount)
    expect(typeof all[0].amount).toBe('bigint')
  })

  it('updates status', async () => {
    const store = new ClaimLinkStore()
    await store.load()
    const rec = await store.add(SAMPLE)
    await store.setStatus(rec.id, 'claimed')
    expect(store.getAll()[0].status).toBe('claimed')
  })

  it('deletes records', async () => {
    const store = new ClaimLinkStore()
    await store.load()
    const rec = await store.add(SAMPLE)
    await store.delete(rec.id)
    expect(store.getAll()).toHaveLength(0)
  })

  it('sorts newest first', async () => {
    const store = new ClaimLinkStore()
    await store.load()
    await store.add({ ...SAMPLE, createdAt: 100 })
    await store.add({ ...SAMPLE, ephAddress: 'tbotho://1/newer', createdAt: 200 })
    const all = store.getAll()
    expect(all[0].createdAt).toBe(200)
    expect(all[1].createdAt).toBe(100)
  })

  it('detects expiry against the UX window', async () => {
    const store = new ClaimLinkStore()
    const now = 1_000_000
    const fresh: ClaimLinkRecord = {
      id: 'a', ...SAMPLE, createdAt: now - 10, status: 'outstanding',
    }
    const old: ClaimLinkRecord = {
      id: 'b', ...SAMPLE, createdAt: now - CLAIM_LINK_EXPIRY_WINDOW_SECONDS - 1, status: 'outstanding',
    }
    expect(store.isExpired(fresh, now)).toBe(false)
    expect(store.isExpired(old, now)).toBe(true)
  })

  it('tolerates corrupt localStorage data', async () => {
    localStorage.setItem('botho-claim-links', 'not json')
    const storage = new LocalStorageClaimLinks()
    expect(await storage.load()).toEqual([])
  })
})
