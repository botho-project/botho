/**
 * @vitest-environment jsdom
 *
 * Tests for the wallet LOCK + auto-lock-on-inactivity feature (#490).
 *
 * Like wallet-password.test.tsx (and unlike wallet.test.tsx) this file does NOT
 * mock @botho/core's AddressBook / ClaimLinkStore: the whole security point of
 * locking is that the decrypted seed + session vault key are wiped from memory
 * so the REAL encrypted address book / claim-link blobs become unreadable until
 * an unlock re-derives the key. Only the network adapter and wasm signer are
 * stubbed.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, waitFor, act, cleanup } from '@testing-library/react'
import { WalletProvider, useWallet } from './wallet'

// Stub the wasm signer so the module import is hermetic (no wasm artifact on a
// fresh checkout). None of these tests exercise real signing/scanning.
vi.mock('@botho/wasm-signer', () => ({
  deriveV2Address: vi.fn(async (mnemonic: string) => {
    // Deterministic per-mnemonic stand-in for the wasm v2 deriver: base58-only
    // body so `/^tbotho:\/\/2\/[A-Za-z1-9]+$/` matches and different
    // mnemonics yield different addresses.
    const b58 = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz123456789'
    let acc = ''
    for (let i = 0; i < mnemonic.length && acc.length < 40; i++) {
      acc += b58[mnemonic.charCodeAt(i) % b58.length]
    }
    return 'tbotho://2/' + (acc || '11111')
  }),
  spendableBalance: vi.fn().mockResolvedValue(0n),
  buildOwnedHistory: vi.fn().mockResolvedValue([]),
  buildSendTransaction: vi.fn().mockResolvedValue({ txHex: '0xstub' }),
  deriveKemPublicKey: vi.fn().mockResolvedValue('00'.repeat(1184)),
}))

// Real-backing localStorage mock so the encrypted stores actually persist blobs
// we can inspect across lock/unlock.
const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => { store[key] = value },
    removeItem: (key: string) => { delete store[key] },
    clear: () => { store = {} },
    _dump: () => ({ ...store }),
  }
})()
Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

// Stub the adapter. isConnected returns false so unlock/create do not attempt a
// balance fetch (which would reach the stubbed wasm path); the lock-state
// assertions do not depend on balances.
vi.mock('@botho/adapters', () => ({
  RemoteNodeAdapter: class MockRemoteNodeAdapter {
    connect = vi.fn().mockResolvedValue(undefined)
    disconnect = vi.fn()
    isConnected = vi.fn().mockReturnValue(false)
    getNodeInfo = vi.fn().mockReturnValue({ version: '1.0.0', network: 'testnet' })
    getBalance = vi.fn().mockResolvedValue({ available: 0n, pending: 0n, total: 0n })
    getTransactionHistory = vi.fn().mockResolvedValue([])
    onNewBlock = vi.fn().mockReturnValue(() => {})
    onTransaction = vi.fn().mockReturnValue(() => {})
    onMempoolUpdate = vi.fn().mockReturnValue(() => {})
    onPeerStatus = vi.fn().mockReturnValue(() => {})
    getWsStatus = vi.fn().mockReturnValue('disconnected')
    onWsStatusChange = vi.fn().mockReturnValue(() => {})
  },
}))

const TEST_MNEMONIC_12 =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'
const A_CONTACT = 'tbotho://1/contactaddrabc'

const STORAGE_AUTO_LOCK = 'botho-auto-lock-minutes'

function Harness({ onMount }: { onMount: (w: ReturnType<typeof useWallet>) => void }) {
  const wallet = useWallet()
  onMount(wallet)
  return null
}

async function mountWallet(): Promise<{ get: () => ReturnType<typeof useWallet>; unmount: () => void }> {
  let ref: ReturnType<typeof useWallet> | null = null
  const utils = render(
    <WalletProvider>
      <Harness onMount={(w) => { ref = w }} />
    </WalletProvider>
  )
  await waitFor(() => expect(ref).not.toBeNull())
  return { get: () => ref!, unmount: utils.unmount }
}

describe('wallet lock (#490)', () => {
  beforeEach(() => {
    localStorageMock.clear()
    vi.clearAllMocks()
    vi.useRealTimers()
  })
  afterEach(() => {
    cleanup()
    localStorageMock.clear()
    vi.useRealTimers()
  })

  it('lockWallet clears the in-memory seed + vault key and sets isLocked', async () => {
    const { get } = await mountWallet()

    await act(async () => { await get().createWallet(TEST_MNEMONIC_12, 'hunter2pw') })
    expect(get().isEncrypted).toBe(true)
    expect(get().isLocked).toBe(false)
    // Seed + session key are present while unlocked.
    expect(await get().exportWallet()).toBe(TEST_MNEMONIC_12)
    expect(get().getVaultKey()).not.toBeNull()

    await act(async () => { get().lockWallet() })

    // Locked state is set.
    expect(get().isLocked).toBe(true)
    // SECURITY: the session vault key is wiped from memory.
    expect(get().getVaultKey()).toBeNull()
    // SECURITY: the decrypted seed is wiped — exportWallet now falls back to
    // disk, which for an encrypted wallet requires a password. Without one it
    // cannot return the seed (it throws "Password required to unlock wallet"),
    // proving the cleartext mnemonic is no longer held in memory.
    await expect(get().exportWallet()).rejects.toThrow(/password required/i)
    // The wallet still exists on disk (NOT wiped) — encrypted seed blob remains.
    expect(localStorage.getItem('botho-wallet-mnemonic')).not.toBeNull()
    expect(localStorage.getItem('botho-wallet-encrypted')).toBe('true')
  })

  it('after lock, encrypted contacts are unreadable until unlock restores access', async () => {
    const { get } = await mountWallet()

    await act(async () => { await get().createWallet(TEST_MNEMONIC_12, 'hunter2pw') })
    await act(async () => { await get().addContact('Alice', A_CONTACT) })
    expect(get().contacts.map((c) => c.name)).toContain('Alice')

    // Lock: contacts disappear from the context (the encrypted store reads as
    // empty with no session key).
    await act(async () => { get().lockWallet() })
    expect(get().isLocked).toBe(true)
    expect(get().contacts).toHaveLength(0)
    // The on-disk address-book blob still exists (not wiped).
    expect(localStorage.getItem('botho-address-book')).not.toBeNull()

    // Unlock: the key is re-derived and the contact reloads.
    await act(async () => { await get().unlockWallet('hunter2pw') })
    expect(get().isLocked).toBe(false)
    expect(get().getVaultKey()).not.toBeNull()
    expect(await get().exportWallet()).toBe(TEST_MNEMONIC_12)
    await waitFor(() => {
      expect(get().contacts.map((c) => c.name)).toContain('Alice')
    })
  })

  it('lockWallet is a NO-OP for a plaintext wallet (never strands it)', async () => {
    const { get } = await mountWallet()

    // Plaintext wallet: no password => no session vault key.
    await act(async () => { await get().createWallet(TEST_MNEMONIC_12) })
    expect(get().isEncrypted).toBe(false)
    expect(get().getVaultKey()).toBeNull()
    expect(await get().exportWallet()).toBe(TEST_MNEMONIC_12)

    // Attempting to lock must NOT change state — the wallet has no way to unlock.
    await act(async () => { get().lockWallet() })
    expect(get().isLocked).toBe(false)
    // Seed is still in memory and accessible (not stranded).
    expect(await get().exportWallet()).toBe(TEST_MNEMONIC_12)
  })

  it('setAutoLockMinutes persists the preference; off (0) disables it', async () => {
    const { get } = await mountWallet()

    // Default preference is the sensible non-zero default (15 min).
    expect(get().autoLockMinutes).toBeGreaterThan(0)

    await act(async () => { get().setAutoLockMinutes(5) })
    expect(get().autoLockMinutes).toBe(5)
    expect(localStorage.getItem(STORAGE_AUTO_LOCK)).toBe('5')

    await act(async () => { get().setAutoLockMinutes(0) })
    expect(get().autoLockMinutes).toBe(0)
    expect(localStorage.getItem(STORAGE_AUTO_LOCK)).toBe('0')
  })
})

describe('wallet auto-lock timer (#490)', () => {
  beforeEach(() => {
    localStorageMock.clear()
    vi.clearAllMocks()
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.runOnlyPendingTimers()
    vi.useRealTimers()
    cleanup()
    localStorageMock.clear()
  })

  it('fires after the configured idle timeout and locks the wallet', async () => {
    let ref: ReturnType<typeof useWallet> | null = null
    await act(async () => {
      render(
        <WalletProvider>
          <Harness onMount={(w) => { ref = w }} />
        </WalletProvider>
      )
    })
    const get = () => ref!

    // 1-minute auto-lock, encrypted wallet so locking is allowed.
    await act(async () => { get().setAutoLockMinutes(1) })
    await act(async () => { await get().createWallet(TEST_MNEMONIC_12, 'hunter2pw') })
    expect(get().isLocked).toBe(false)

    // Advance just under the timeout: still unlocked.
    await act(async () => { vi.advanceTimersByTime(59_000) })
    expect(get().isLocked).toBe(false)

    // Cross the threshold: auto-lock fires.
    await act(async () => { vi.advanceTimersByTime(2_000) })
    expect(get().isLocked).toBe(true)
    expect(get().getVaultKey()).toBeNull()
  })

  it('user activity resets the idle timer so it does not fire early', async () => {
    let ref: ReturnType<typeof useWallet> | null = null
    await act(async () => {
      render(
        <WalletProvider>
          <Harness onMount={(w) => { ref = w }} />
        </WalletProvider>
      )
    })
    const get = () => ref!

    await act(async () => { get().setAutoLockMinutes(1) })
    await act(async () => { await get().createWallet(TEST_MNEMONIC_12, 'hunter2pw') })

    // Advance most of the way, then simulate activity to reset the countdown.
    await act(async () => { vi.advanceTimersByTime(50_000) })
    expect(get().isLocked).toBe(false)
    await act(async () => {
      window.dispatchEvent(new Event('keydown'))
    })

    // Another 50s (total 100s from start) — would have fired without the reset,
    // but the reset pushed the deadline out, so still unlocked.
    await act(async () => { vi.advanceTimersByTime(50_000) })
    expect(get().isLocked).toBe(false)

    // Now let the full fresh interval elapse: it fires.
    await act(async () => { vi.advanceTimersByTime(11_000) })
    expect(get().isLocked).toBe(true)
  })

  it('does not auto-lock a plaintext wallet even with a timeout set', async () => {
    let ref: ReturnType<typeof useWallet> | null = null
    await act(async () => {
      render(
        <WalletProvider>
          <Harness onMount={(w) => { ref = w }} />
        </WalletProvider>
      )
    })
    const get = () => ref!

    await act(async () => { get().setAutoLockMinutes(1) })
    // Plaintext wallet (no password).
    await act(async () => { await get().createWallet(TEST_MNEMONIC_12) })

    await act(async () => { vi.advanceTimersByTime(120_000) })
    // Never locked — a plaintext wallet has nothing to unlock with.
    expect(get().isLocked).toBe(false)
    expect(await get().exportWallet()).toBe(TEST_MNEMONIC_12)
  })

  it('off (0) disables auto-lock entirely', async () => {
    let ref: ReturnType<typeof useWallet> | null = null
    await act(async () => {
      render(
        <WalletProvider>
          <Harness onMount={(w) => { ref = w }} />
        </WalletProvider>
      )
    })
    const get = () => ref!

    await act(async () => { get().setAutoLockMinutes(0) })
    await act(async () => { await get().createWallet(TEST_MNEMONIC_12, 'hunter2pw') })

    await act(async () => { vi.advanceTimersByTime(10 * 60_000) })
    expect(get().isLocked).toBe(false)
  })
})
