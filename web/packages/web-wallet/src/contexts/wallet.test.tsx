/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest'
import { render, screen, waitFor, act, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { WalletProvider, useWallet } from './wallet'

// Mock @botho/wasm-signer so these context tests never depend on the generated
// wasm artifact (`packages/wasm-signer/pkg/`, produced by `build:wasm` and
// git-ignored). The wallet context imports buildSendTransaction /
// spendableBalance / buildOwnedHistory at module load; on a fresh checkout the
// wasm pkg is absent, so anything that reaches those functions would otherwise
// blow up with "wasm artifact not found" and poison the suite. None of the
// assertions here exercise real signing/scanning (the adapter is mocked), so we
// substitute inert stubs that keep the import hermetic and the suite fast.
// buildSendTransaction is only ever reached via the encrypted-wallet claim-link
// happy path, which is itself mocked at ../lib/claim-link-ops, so it is never
// actually invoked — it is stubbed purely to satisfy the import.
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
  // No owned outputs => zero spendable balance; the context falls back to the
  // mocked adapter balance, which is what the tests assert against.
  spendableBalance: vi.fn().mockResolvedValue(0n),
  // No owned outputs => empty client-side history.
  buildOwnedHistory: vi.fn().mockResolvedValue([]),
  buildSendTransaction: vi.fn().mockResolvedValue({ txHex: '0xstub' }),
  // Sender's own ML-KEM key for change encapsulation (#978); stubbed.
  deriveKemPublicKey: vi.fn().mockResolvedValue('00'.repeat(1184)),
}))

// Mock localStorage
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

// Mock the RemoteNodeAdapter
vi.mock('@botho/adapters', () => ({
  RemoteNodeAdapter: class MockRemoteNodeAdapter {
    connect = vi.fn().mockResolvedValue(undefined)
    disconnect = vi.fn()
    isConnected = vi.fn().mockReturnValue(true)
    getNodeInfo = vi.fn().mockReturnValue({ version: '1.0.0', network: 'testnet' })
    getBalance = vi.fn().mockResolvedValue({ available: 1000000000000n, pending: 0n, total: 1000000000000n })
    getTransactionHistory = vi.fn().mockResolvedValue([])
    onNewBlock = vi.fn().mockReturnValue(() => {})
    onTransaction = vi.fn().mockReturnValue(() => {})
    onMempoolUpdate = vi.fn().mockReturnValue(() => {})
    onPeerStatus = vi.fn().mockReturnValue(() => {})
    getWsStatus = vi.fn().mockReturnValue('connected')
    onWsStatusChange = vi.fn().mockReturnValue(() => {})
  },
}))

// Mock AddressBook
vi.mock('@botho/core', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@botho/core')>()
  return {
    ...actual,
    AddressBook: class MockAddressBook {
      load = vi.fn().mockResolvedValue(undefined)
      getAll = vi.fn().mockReturnValue([])
      add = vi.fn().mockResolvedValue({ id: '1', name: 'Test', address: 'tbotho://1/test' })
      update = vi.fn().mockResolvedValue({ id: '1', name: 'Updated', address: 'tbotho://1/test' })
      delete = vi.fn().mockResolvedValue(undefined)
      getDisplayName = vi.fn().mockReturnValue('Unknown')
      findByAddress = vi.fn().mockReturnValue(undefined)
      recordTransaction = vi.fn().mockResolvedValue(undefined)
      search = vi.fn().mockReturnValue([])
    },
  }
})

// Mock the on-chain claim-link operations so we can assert whether a funding
// transaction was ever submitted. buildAndSubmitSend is the on-chain spend that
// must NOT run for a plaintext (no-password) wallet (fund-loss guard, #474).
const buildAndSubmitSendMock = vi.fn().mockResolvedValue('0xfundinghash')
vi.mock('../lib/claim-link-ops', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../lib/claim-link-ops')>()
  return {
    ...actual,
    buildAndSubmitSend: (...args: unknown[]) => buildAndSubmitSendMock(...args),
  }
})

const TEST_MNEMONIC_12 = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'

// Test component to access wallet context
function TestConsumer({ onMount }: { onMount?: (wallet: ReturnType<typeof useWallet>) => void }) {
  const wallet = useWallet()

  if (onMount) {
    onMount(wallet)
  }

  return (
    <div>
      <div data-testid="hasWallet">{wallet.hasWallet ? 'yes' : 'no'}</div>
      <div data-testid="address">{wallet.address ?? 'none'}</div>
      <div data-testid="isConnecting">{wallet.isConnecting ? 'yes' : 'no'}</div>
      <div data-testid="isLocked">{wallet.isLocked ? 'yes' : 'no'}</div>
      <div data-testid="isEncrypted">{wallet.isEncrypted ? 'yes' : 'no'}</div>
      <button data-testid="createWallet" onClick={() => wallet.createWallet(TEST_MNEMONIC_12)}>
        Create
      </button>
      <button data-testid="importWallet" onClick={() => wallet.importWallet(TEST_MNEMONIC_12)}>
        Import
      </button>
    </div>
  )
}

describe('WalletContext', () => {
  beforeEach(() => {
    localStorageMock.clear()
    vi.clearAllMocks()
  })

  afterEach(() => {
    // Explicitly unmount rendered trees between tests. Several tests in this
    // file render the WalletProvider multiple times (and one unmounts +
    // re-renders within a single test). Without a deterministic cleanup the
    // mounted <div data-testid="address"> nodes accumulate in the jsdom body
    // and screen.getByTestId('address') throws a "multiple elements" error.
    cleanup()
    vi.clearAllMocks()
  })

  describe('Initial State', () => {
    it('starts without a wallet', async () => {
      render(
        <WalletProvider>
          <TestConsumer />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(screen.getByTestId('hasWallet').textContent).toBe('no')
        expect(screen.getByTestId('address').textContent).toBe('none')
      })
    })

    it('throws error when useWallet is used outside provider', () => {
      const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})

      expect(() => render(<TestConsumer />)).toThrow(
        'useWallet must be used within a WalletProvider'
      )

      consoleError.mockRestore()
    })
  })

  describe('createWallet', () => {
    it('creates a new wallet successfully', async () => {
      const user = userEvent.setup()

      render(
        <WalletProvider>
          <TestConsumer />
        </WalletProvider>
      )

      // Wait for initial connection
      await waitFor(() => {
        expect(screen.getByTestId('isConnecting').textContent).toBe('no')
      })

      // Create wallet
      await user.click(screen.getByTestId('createWallet'))

      await waitFor(() => {
        expect(screen.getByTestId('hasWallet').textContent).toBe('yes')
        expect(screen.getByTestId('address').textContent).not.toBe('none')
        expect(screen.getByTestId('address').textContent).toMatch(/^tbotho:\/\/2\//)
      })
    })

    it('throws error for invalid mnemonic', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      await expect(walletRef!.createWallet('invalid mnemonic words')).rejects.toThrow(
        'Invalid mnemonic provided'
      )
    })
  })

  describe('importWallet', () => {
    it('imports a wallet with valid 12-word mnemonic', async () => {
      const user = userEvent.setup()

      render(
        <WalletProvider>
          <TestConsumer />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(screen.getByTestId('isConnecting').textContent).toBe('no')
      })

      await user.click(screen.getByTestId('importWallet'))

      await waitFor(() => {
        expect(screen.getByTestId('hasWallet').textContent).toBe('yes')
        expect(screen.getByTestId('address').textContent).toMatch(/^tbotho:\/\/2\//)
      })
    })

    it('normalizes mnemonic input (whitespace, case)', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      // Import with messy whitespace and mixed case
      const messyMnemonic = '  ABANDON  abandon  ABANDON   abandon abandon abandon abandon abandon abandon abandon abandon ABOUT  '

      await act(async () => {
        await walletRef!.importWallet(messyMnemonic)
      })

      expect(screen.getByTestId('hasWallet').textContent).toBe('yes')
    })

    it('throws error for invalid word count', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      await expect(walletRef!.importWallet('only three words')).rejects.toThrow(
        'Invalid recovery phrase. Expected 12 or 24 words.'
      )
    })

    it('throws error for invalid BIP39 words', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      // Exactly 12 words but not valid BIP39 words
      const invalidMnemonic = 'invalid words that are not in the bip39 wordlist at all now'
      await expect(walletRef!.importWallet(invalidMnemonic)).rejects.toThrow(
        'Invalid recovery phrase. Please check your words and try again.'
      )
    })
  })

  describe('Wallet with Password', () => {
    it('creates encrypted wallet', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      await act(async () => {
        await walletRef!.createWallet(TEST_MNEMONIC_12, 'mypassword')
      })

      expect(screen.getByTestId('hasWallet').textContent).toBe('yes')
      expect(screen.getByTestId('isEncrypted').textContent).toBe('yes')
      expect(screen.getByTestId('isLocked').textContent).toBe('no') // Not locked after creation
    })

    it('imports wallet with password protection', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      await act(async () => {
        await walletRef!.importWallet(TEST_MNEMONIC_12, 'securepassword')
      })

      expect(screen.getByTestId('isEncrypted').textContent).toBe('yes')
    })
  })

  describe('address book wiring', () => {
    it('exposes recordPayment and searchContacts on the context', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      expect(typeof walletRef!.recordPayment).toBe('function')
      expect(typeof walletRef!.searchContacts).toBe('function')

      // recordPayment is a non-throwing upsert; searchContacts returns an array.
      await act(async () => {
        await walletRef!.recordPayment('tbotho://1/test')
      })
      expect(Array.isArray(walletRef!.searchContacts('test'))).toBe(true)
    })
  })

  describe('exportWallet', () => {
    it('exports mnemonic after wallet creation', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      await act(async () => {
        await walletRef!.createWallet(TEST_MNEMONIC_12)
      })

      const exported = await walletRef!.exportWallet()
      expect(exported).toBe(TEST_MNEMONIC_12)
    })
  })

  describe('sendViaLink fund-loss guard (#474)', () => {
    it('on a plaintext (no-password) wallet, throws BEFORE funding — no on-chain spend', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      // Create a plaintext wallet (no password => no session vault key).
      await act(async () => {
        await walletRef!.createWallet(TEST_MNEMONIC_12)
      })
      expect(screen.getByTestId('isEncrypted').textContent).toBe('no')

      buildAndSubmitSendMock.mockClear()

      // Attempting to create a claim link must fail fast with an actionable
      // message that routes the user to add a password — and must NOT spend.
      await expect(walletRef!.sendViaLink(1_000_000n)).rejects.toThrow(
        /password-protected wallet/i,
      )

      // The on-chain funding spend must never have been invoked.
      expect(buildAndSubmitSendMock).not.toHaveBeenCalled()

      // No outstanding link should have been recorded either.
      expect(walletRef!.claimLinks.length).toBe(0)
    })

    it('on an encrypted wallet, funds and returns a claim link (happy path)', async () => {
      let walletRef: ReturnType<typeof useWallet> | null = null

      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )

      await waitFor(() => {
        expect(walletRef).not.toBeNull()
      })

      await act(async () => {
        await walletRef!.createWallet(TEST_MNEMONIC_12, 'mypassword')
      })
      expect(screen.getByTestId('isEncrypted').textContent).toBe('yes')

      buildAndSubmitSendMock.mockClear()

      let created: Awaited<ReturnType<ReturnType<typeof useWallet>['sendViaLink']>> | null = null
      await act(async () => {
        created = await walletRef!.sendViaLink(1_000_000n)
      })

      expect(buildAndSubmitSendMock).toHaveBeenCalledTimes(1)
      expect(created).not.toBeNull()
      expect(created!.url).toContain('/claim')
      expect(created!.amount).toBe(1_000_000n)
      expect(created!.fundingTxHash).toBe('0xfundinghash')
    })
  })
})

describe('Import Wallet Integration', () => {
  beforeEach(() => {
    // Unmount any tree left over from the previous test/suite so the
    // data-testid="address" query stays unambiguous (see note above).
    cleanup()
    localStorageMock.clear()
  })

  it('full import flow works correctly', async () => {
    let walletRef: ReturnType<typeof useWallet> | null = null

    render(
      <WalletProvider>
        <TestConsumer onMount={(w) => { walletRef = w }} />
      </WalletProvider>
    )

    await waitFor(() => {
      expect(walletRef).not.toBeNull()
    })

    // Start without wallet
    expect(screen.getByTestId('hasWallet').textContent).toBe('no')

    // Import wallet
    await act(async () => {
      await walletRef!.importWallet(TEST_MNEMONIC_12)
    })

    // Wallet should now exist
    expect(screen.getByTestId('hasWallet').textContent).toBe('yes')
    expect(screen.getByTestId('address').textContent).toMatch(/^tbotho:\/\/2\/[A-Za-z1-9]+$/)

    // Should be able to export it
    const exported = await walletRef!.exportWallet()
    expect(exported).toBe(TEST_MNEMONIC_12)
  })

  it('address is deterministic from mnemonic', async () => {
    let walletRef: ReturnType<typeof useWallet> | null = null

    const { unmount } = render(
      <WalletProvider>
        <TestConsumer onMount={(w) => { walletRef = w }} />
      </WalletProvider>
    )

    await waitFor(() => {
      expect(walletRef).not.toBeNull()
    })

    await act(async () => {
      await walletRef!.importWallet(TEST_MNEMONIC_12)
    })

    const address1 = screen.getByTestId('address').textContent

    // Clear and reimport - should get same address
    unmount()
    localStorageMock.clear()

    render(
      <WalletProvider>
        <TestConsumer onMount={(w) => { walletRef = w }} />
      </WalletProvider>
    )

    await waitFor(() => {
      expect(walletRef).not.toBeNull()
    })

    await act(async () => {
      await walletRef!.importWallet(TEST_MNEMONIC_12)
    })

    const address2 = screen.getByTestId('address').textContent

    expect(address1).toBe(address2)
  })
})
