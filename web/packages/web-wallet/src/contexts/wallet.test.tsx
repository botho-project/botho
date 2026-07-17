/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest'
import { render, screen, waitFor, act, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { WalletProvider, useWallet } from './wallet'
import { buildSendTransaction } from '@botho/wasm-signer'

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
  netOwnedHistory: vi.fn(() => []),
  ownedOutputTargetKeys: vi.fn().mockResolvedValue([]),
  mnemonicToSeedHex: vi.fn(() => '00'.repeat(64)),
  // Captured so the #1037 send-channel test can assert what the signer received.
  // Returns a valid-hex tx so `hexToBytes(txHex)` in `send()` succeeds.
  buildSendTransaction: vi.fn().mockResolvedValue({ txHex: 'ab' }),
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
    // Send-path methods (#1037 channel test). buildSendTransaction is mocked, so
    // the fee/cluster/rpc calls only need to resolve; submitTransaction returns a
    // success so `send()` reaches its return.
    getBlockHeight = vi.fn().mockResolvedValue(100)
    getRawOutputs = vi.fn().mockResolvedValue([])
    areKeyImagesSpent = vi.fn().mockResolvedValue([])
    getClusterWealth = vi.fn().mockResolvedValue(0n)
    estimateFee = vi.fn().mockResolvedValue({ fee: 100_000_000n })
    submitTransaction = vi.fn().mockResolvedValue({ success: true, txHash: '0xhash' })
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

  // #1037: `send(to, amount, options)` carries two DISTINCT channels — a human
  // free-text `note` (cosmetic, dropped, never on-chain) and the bridge
  // `bridgeDepositMemo` (64-byte hex, embedded on-chain). Before the fix the
  // single `memo` arg was routed straight into the signer's strict 64-byte-hex
  // validator, so a plain note like "lunch" made the transaction fail
  // ("memo: invalid hex"). These tests lock the channels apart.
  describe('send memo channels (#1037)', () => {
    // Canonical testnet v2 address (the node's derivation for the abandon…about
    // mnemonic) so the real `parseAddress` in `send()` succeeds.
    const GOLDEN_TESTNET_ADDRESS =
      'tbotho://2/apm8kqabmyjn7V6R35S7ZN2bAYoupZGbqQX4u5H62LRih6vDZu2ZvAAJXiUSXN6amQPusu9gEGwWwP22fdeJeU4PZ5ikmjKB8FfgeQjEEBnqr117z2pxXo96jLEH2Q7HjxFWkHLCTzLKmkHhQPL2c5uSCAxzrSXRZkdvGsFfy7kkC2YR5121SgghjSFzX3ThA2Jf6xU9ZMwHxDJbHHA7GhytiB7197BNNhicsTQ26GAUDEAikKPFV2pkWUqnXHF7zX9fRfvTURUZqrGeWskvYCKTngN753rJtjMWwoqCVs2MmaJAnrfTb56rpwNHZiQgbhF7S4zFVYXYwUUSs1uJYCZrUZ2qJeZC4USMX9MCkzJbuizJHeNoWVKfe1eqdkLiX9TQBU1W44QdnNM6N65ve157hDe6ATUD4K8FiqARLz8VgMRvj1Ai4xYDaJGo38oC1d2dSunNwkUYMr6ucFyF2fv4WtAXgNUaDxsFT8Hpxqd4mTGjBKA7dBNtWQfn1zAQHqKZpehaPE3eHP3MujYi62iHme1JkcRpbwjAuLPXWRrD8WBbzVctrsMbnRwVE8uttAbX3CHRwVHBDodUWGXyRGib1wocMLW3WrnRGXdnnEtD7PzLK2rJ25zdm8ZTyMSE1aEPpp5W7HKLv8iFkWhitCDasUagv7VGkNjizp26rFpm3osimjUapAVSY1ShrAXeqbw5PWH3nS5KXmUtPivB1FNifGcTm3Soiziq4twPqBKmMey3vTU6edULkZB1wiXWEeFmJ3uziqFKfitzjYYsgbk5mgTetzTgx13WrAhwfJK5rFkU3ZGKPVEdg6ARB94UGXZWScNdi8S1fspJoaTvDydMmFrgvsGHYVpFfNyjYwU7uD9aVfVHsEwwoPQt9KNGoZbUxLBJ5VzbkaRaers8zRtU25yx8Qakr5oTZFwbA4xnGtuSCXqaEiu2dNnha6RXSvJ5J8XsnGydCvvqW1UfkjdpNVGbgPDh1Vc77T46zDE1VaTGjKxwBoiqMz6YaUF61nCXXcJL3QeDDs8HZH6DBeD6HEwHQoCJRNZ9VGNiFx3699yziLuJXHECTRbwpdqy3ybhWrjL43xTPJnJRXY62ye3XpWPUH4Wmyns8WYzBS3T6ay9Xyf4zrFTC8MUYyCMUfPFqwak1jZ7uh7JYvGuLGcd1TjV3sgngmy5nx349rQAZXUhapTFF5QtdePXPunQxK6QzvGqk7Aeuxtr317NxT77M1AMHC3N9zbMtjaKypUW5RrkJfuTA3S2h45Cz4n74fbmrRc54TktsonGkMzNTTkQre79sQRZXgQtXYNghrPyNzEmo8CTwUMkThw3fkkPfVppaw9VPZpoqonQLG8SCNmwj9YteUM5BYWKQvoRMXy4kBW95jMCPXXkXjkjFLC1qpCiRjXUZd6wd64u2sXKfLJ9fMgz54NknAuYiZawUZLWWkehkdfgwpeXgWd48FL6ZmV2xBZmA3s8JGPiYbAqRYwNySKsKNaDNd7rsh4xbKb2QR6vVuBkgrryVgAKeTymf8hEAE3tpq9puKhu1jGp4q61n6SUyWsXka8ekrjyJVFNXXPF6XpFGC6D5wY8n9kzVgq6kupxZAep4qyFHtudFcqRzNwm1eqeFoRxuCZhyyFFynGJTMWgLBTQ1oQQSX7gD3h2bFFGxtcZp8EEdFZG4faZwXgw79qz1Ft5N2FwfGKwjBcjWVAdVH85Fm7piKdoMSnfwBAqfLPhPQd31qvNZ7F4abRcMTUhjMPDoWD5g8TbDSnQcj9fcw8uE4s7C59xufYibpKxBP4o52pe8pWaNsCrjC6xG2Ss7ZZuU8z2X9sVxkaw37opneLkbwd9CMLLvEYFzQJMv32DM66m6HnKgijtwgUBbyrgYwwc7UzPVteG4VCjMnoeQbAWPNEVDZmnQ5k2HtpEa6K1jvKUS2zT3gxoktmBD17d9a9GzwjMB8jDBn9vRqTogzaJLXyNBbAkTmrZWJoVDNSqEYZQntX3RsCnTVdauCXnFx11NTpZ9tFoKeoQs9r7N6u1ur2Rg5mhRoGUy6iCm4Ndfj4ukNXh1heSUf4cTHDeHTCeeSQbquYNYR7VtagLnGEogA2neETexLzuYtDpE5UsCmJW6JD3tXmL5vtwUJgiwzgHoRUgCyFTQcX5sRax27AWewwmzWUBH1A374S3CogyunE6Ar1Mdoq98rgtNZyctjXXkBxfrF2wQ7rV4dTpAWx8vNVzdMLvQYqipkdWRG7NKrLtfe2VndZvULuQpDJT1QcjqXFb65DeEhbsJGZQ2GyqBNnKDPCNA8ToHYJG3UgePqqT5vZqz4eo29heS1i1raFmeadkFss2HtY6PToVXPJGcJRT9dSHPHD4Mab4TcfziydCgPiWEVQMbxdVV34K8K81gEjjMFr3bs66beHMgB54mCm4pd9rYQaQY63bfHT1Hk6nzD2TvAsVCW6hCcpja5vYxMTy1BJ1LXqfvrysvwDsTKHcqK1eicoJhqiCV48PeLthsdfPDNJP3ynsQvYoN66RVZHSRN7K63u75ZANiMDCFSZN7M4c5WyoLTyWHcB7Q2F93c2HEHBjNuuJQHmhXt3sHccG36ejcZtzcnBcmRSSZ5bHNEDo2oJuuiyRQnCEQ29CCwoGwunxNijkuD75rDdiBUedKZjyaqjAS4sZJZeHe6NCMEDzh4qgTUDUmjgZz1dyx7S45RHsdB7CEbnyiDeBYEinwTtEqQbPKJY6FJwQ4ctbovNTGZBRdhLFQD5DwEHHk7Fmmm3pfXF8cZB9qhTFYstVBJaGUfUGLHwGuzqH8qeEJ8sec9F4JZAr3DrVY42krBBJDiMPru866tZkqjGvzQLP8xQz2XcgcvDwWuEsbF34bTFi5HzfreycywGoLfcN2L69FEny4knrbSpNCmn8qXcV1k5qq6WkLwr9SneY2BEKZxd29Ug5fpbtGoT3jjxbVccNuEhQDqJ2TxQzEgXTPBFZbgYMVo8HxFrqGczMwwZMDTeMRMGS7Br8rsmU45zk1rR22aWho1yTTwD2WGhpcPuxvqjWfy93gvmEoRS1nT8MvDvCktRkXwCtoCoRz4ZQwE5L3JzhtFzh3Vmo8Gnqj7jqxHKDmCfjPqd4DZrySucCcKYs1yr4Nk59T8DSzmZ7J2HdnRUPuD3yLt2tMVNDVESaNkviyVmCdb2R5N67XKfhDAxp3v3UzF1sbFHLYH7tgfKQw6rtiCT7g4ZHGafawwZDZb1dgJByDkCaSjdaiKCB7aQMKmjq9g6gAMcwD1pbGz9FvbhWL25n6J7m2DhLDCo5fnQR4FVqaNoB5trVc2wv2X96RcWzZp4QQPAum7p7CBk6kwRqr8sGdvh3ko55WvYTZG3oDjAd3VfScRTFMo3BTTrNgVJpwAdWZsnikL2ewKyLDjHxKPnB5EB3iGbKXrjkrgPMJ3M7spB3T3HqVafXmgfyKmwuPhvEUNn77mr4X8dsDMGnLB3vaeQRZP1RSAWFgZusDFKvYY8JwFcEBswUb5JgjSNvUSU2T5pFcc8mqTB53nAzCNoHvX5FBnQCXrWpN3E359PBiiuJmeBdQhjWh7yCh3NzdxNhKcsySgiNDUXkQSnGirmPMMiwKrXRrv6XXFyyxsdBErWGKzc2cDqYbzygiyBYeYk8SB9cJioJ6S4DRbZs7NxY2jsNK9JUPGDeUeVuDNdVwSuBucwDhjeCBtSzUCQy8ET6Lyu6SduG6cF9xb6JC693nsgx7VENQApY3Bc8bLkKDXz8JYuSXnQbSouXLgperYLQS3rjd3ZLUmKYyW3pr75R79ZEwftqtNQzRaPNAohCBJaCvA2BdG8SPaWbJZ6UdSxLzm3tJ1uYC2kaHhzub4jmnYAGuCVHLBwbppShmsVKxnfUyo2L8fDguzPc2kr6CBjcsxLgBaAUiE9tk2S6DDngBZ3f3UNdrv4yy5dp2p1nWHN9FRtVgybRvCYvdm8wspsRWH23muUN2FrGgFV5WTeigrgoQ8DRZpn7zpLYwMvwzbtZFWHcN5b9xFbNnzh27A3X5eGuvF1tbUXA21Rk2Ey9myfdSrDxpxFkFWNbdxpVLoSdDKLu5Frq9yrvxafjwP3DvBCZsvZ4gpDva7qSGaEEXnuDPycPWBgUpwYpDs1vPdBAef1U3EvznqZ6HTgG9C8znD47E9MURoaxwwxZq448cEdHtor9Rdz7iCHTrsJW9PxyVB9cnsze1tdYPzBMwvd1tts8r27nzWegNhUpDr8D2E8qBqkPLU4UZzfz1gL5roGdNyt5dQSeig1JsBU62imeYQ753eNCbiLW8uvcKyJKCBwoYVVB46UAoc3nfAGa1vfboZYBNp9AovLsqvn5JxiNxu2cLNNSA9'

    async function mountWithWallet(): Promise<ReturnType<typeof useWallet>> {
      let walletRef: ReturnType<typeof useWallet> | null = null
      render(
        <WalletProvider>
          <TestConsumer onMount={(w) => { walletRef = w }} />
        </WalletProvider>
      )
      await waitFor(() => { expect(walletRef).not.toBeNull() })
      await act(async () => { await walletRef!.createWallet(TEST_MNEMONIC_12) })
      return walletRef!
    }

    it('a free-text note does NOT reach the signer (no regression: "lunch" still sends)', async () => {
      const wallet = await mountWithWallet()
      vi.mocked(buildSendTransaction).mockClear()

      // A human note like "lunch" would fail the signer's 64-byte-hex validator
      // if routed on-chain. It must send successfully and carry NO on-chain memo.
      let txHash: string | undefined
      await act(async () => {
        txHash = await wallet.send(GOLDEN_TESTNET_ADDRESS, 1_000_000n, { note: 'lunch' })
      })

      expect(txHash).toBe('0xhash')
      expect(buildSendTransaction).toHaveBeenCalledTimes(1)
      const arg = vi.mocked(buildSendTransaction).mock.calls[0][0]
      expect(arg.bridgeDepositMemo).toBeUndefined()
    })

    it('the bridge deposit memo IS threaded on-chain (64-byte hex)', async () => {
      const wallet = await mountWithWallet()
      vi.mocked(buildSendTransaction).mockClear()

      const orderMemo = 'deadbeef001122334455667788990011'.padEnd(128, '0')
      await act(async () => {
        await wallet.send(GOLDEN_TESTNET_ADDRESS, 1_000_000n, { bridgeDepositMemo: orderMemo })
      })

      expect(buildSendTransaction).toHaveBeenCalledTimes(1)
      const arg = vi.mocked(buildSendTransaction).mock.calls[0][0]
      expect(arg.bridgeDepositMemo).toBe(orderMemo)
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
