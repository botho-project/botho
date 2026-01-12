/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest'
import { render, screen, waitFor, act } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { WalletProvider, useWallet } from './wallet'

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
    },
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
        expect(screen.getByTestId('address').textContent).toMatch(/^tbotho:\/\/1\//)
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
        expect(screen.getByTestId('address').textContent).toMatch(/^tbotho:\/\/1\//)
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
})

describe('Import Wallet Integration', () => {
  beforeEach(() => {
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
    expect(screen.getByTestId('address').textContent).toMatch(/^tbotho:\/\/1\/[A-Za-z1-9]+$/)

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
