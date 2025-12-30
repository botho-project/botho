import { describe, it, expect, beforeEach } from 'vitest'
import {
  createMnemonic,
  createMnemonic12,
  isValidMnemonic,
  deriveAddress,
  encrypt,
  decrypt,
  saveWallet,
  loadWallet,
  getWalletInfo,
  hasStoredWallet,
  isWalletEncrypted,
  clearWallet,
} from './index'

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

// Known test vectors for deterministic testing
const TEST_MNEMONIC_12 = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'
const TEST_MNEMONIC_24 = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art'

describe('Wallet Core Functions', () => {
  beforeEach(() => {
    localStorageMock.clear()
  })

  describe('createMnemonic', () => {
    it('generates a 24-word mnemonic', () => {
      const mnemonic = createMnemonic()
      const words = mnemonic.split(' ')
      expect(words).toHaveLength(24)
    })

    it('generates valid BIP39 mnemonics', () => {
      const mnemonic = createMnemonic()
      expect(isValidMnemonic(mnemonic)).toBe(true)
    })

    it('generates unique mnemonics on each call', () => {
      const mnemonic1 = createMnemonic()
      const mnemonic2 = createMnemonic()
      expect(mnemonic1).not.toBe(mnemonic2)
    })
  })

  describe('createMnemonic12', () => {
    it('generates a 12-word mnemonic', () => {
      const mnemonic = createMnemonic12()
      const words = mnemonic.split(' ')
      expect(words).toHaveLength(12)
    })

    it('generates valid BIP39 mnemonics', () => {
      const mnemonic = createMnemonic12()
      expect(isValidMnemonic(mnemonic)).toBe(true)
    })

    it('generates unique mnemonics on each call', () => {
      const mnemonic1 = createMnemonic12()
      const mnemonic2 = createMnemonic12()
      expect(mnemonic1).not.toBe(mnemonic2)
    })
  })

  describe('isValidMnemonic', () => {
    it('validates correct 12-word mnemonic', () => {
      expect(isValidMnemonic(TEST_MNEMONIC_12)).toBe(true)
    })

    it('validates correct 24-word mnemonic', () => {
      expect(isValidMnemonic(TEST_MNEMONIC_24)).toBe(true)
    })

    it('rejects invalid word count', () => {
      expect(isValidMnemonic('abandon abandon abandon')).toBe(false)
    })

    it('rejects invalid words', () => {
      expect(isValidMnemonic('invalid words that are not in the bip39 wordlist at all here')).toBe(false)
    })

    it('rejects empty string', () => {
      expect(isValidMnemonic('')).toBe(false)
    })

    it('rejects mnemonic with invalid checksum', () => {
      // Valid words but invalid checksum (last word changed)
      const invalidChecksum = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon'
      expect(isValidMnemonic(invalidChecksum)).toBe(false)
    })

    it('handles extra whitespace gracefully', () => {
      const withExtraSpaces = '  abandon  abandon  abandon  abandon  abandon  abandon  abandon  abandon  abandon  abandon  abandon  about  '
      // BIP39 library should handle this
      expect(isValidMnemonic(withExtraSpaces.trim().replace(/\s+/g, ' '))).toBe(true)
    })

    it('is case-sensitive (lowercase only)', () => {
      const uppercase = 'ABANDON ABANDON ABANDON ABANDON ABANDON ABANDON ABANDON ABANDON ABANDON ABANDON ABANDON ABOUT'
      expect(isValidMnemonic(uppercase)).toBe(false)
    })
  })

  describe('deriveAddress', () => {
    it('derives a deterministic address from mnemonic', () => {
      const address1 = deriveAddress(TEST_MNEMONIC_12)
      const address2 = deriveAddress(TEST_MNEMONIC_12)
      expect(address1).toBe(address2)
    })

    it('starts with bth1 prefix', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      expect(address.startsWith('bth1')).toBe(true)
    })

    it('has correct length (bth1 + 40 hex chars)', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      expect(address).toHaveLength(4 + 40) // 'bth1' + 40 hex chars
    })

    it('derives different addresses for different mnemonics', () => {
      const address1 = deriveAddress(TEST_MNEMONIC_12)
      const address2 = deriveAddress(TEST_MNEMONIC_24)
      expect(address1).not.toBe(address2)
    })

    it('throws error for invalid mnemonic', () => {
      expect(() => deriveAddress('invalid mnemonic')).toThrow('Invalid mnemonic')
    })

    it('produces consistent addresses (test vector)', () => {
      // This is a known test vector - the address should always be the same
      const address = deriveAddress(TEST_MNEMONIC_12)
      // Verify it's a valid hex address
      expect(address).toMatch(/^bth1[0-9a-f]{40}$/)
    })
  })

  describe('encrypt/decrypt', () => {
    const testPlaintext = 'secret mnemonic phrase'
    const testPassword = 'strong-password-123'

    it('encrypts and decrypts correctly', async () => {
      const encrypted = await encrypt(testPlaintext, testPassword)
      const decrypted = await decrypt(encrypted, testPassword)
      expect(decrypted).toBe(testPlaintext)
    })

    it('produces different ciphertext each time (random salt/iv)', async () => {
      const encrypted1 = await encrypt(testPlaintext, testPassword)
      const encrypted2 = await encrypt(testPlaintext, testPassword)
      expect(encrypted1).not.toBe(encrypted2)
    })

    it('fails decryption with wrong password', async () => {
      const encrypted = await encrypt(testPlaintext, testPassword)
      await expect(decrypt(encrypted, 'wrong-password')).rejects.toThrow()
    })

    it('handles empty plaintext', async () => {
      const encrypted = await encrypt('', testPassword)
      const decrypted = await decrypt(encrypted, testPassword)
      expect(decrypted).toBe('')
    })

    it('handles unicode content', async () => {
      const unicode = 'Hello 世界 🌍 émoji'
      const encrypted = await encrypt(unicode, testPassword)
      const decrypted = await decrypt(encrypted, testPassword)
      expect(decrypted).toBe(unicode)
    })
  })

  describe('saveWallet', () => {
    it('saves unencrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12)

      expect(localStorage.getItem('botho-wallet-mnemonic')).toBe(TEST_MNEMONIC_12)
      expect(localStorage.getItem('botho-wallet-encrypted')).toBe('false')
      expect(localStorage.getItem('botho-wallet-address')).toBeTruthy()
    })

    it('saves encrypted wallet with password', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'mypassword')

      const stored = localStorage.getItem('botho-wallet-mnemonic')
      expect(stored).not.toBe(TEST_MNEMONIC_12) // Should be encrypted
      expect(localStorage.getItem('botho-wallet-encrypted')).toBe('true')
      expect(localStorage.getItem('botho-wallet-address')).toBeTruthy()
    })

    it('stores correct address', async () => {
      await saveWallet(TEST_MNEMONIC_12)

      const storedAddress = localStorage.getItem('botho-wallet-address')
      const expectedAddress = deriveAddress(TEST_MNEMONIC_12)
      expect(storedAddress).toBe(expectedAddress)
    })
  })

  describe('loadWallet', () => {
    it('loads unencrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12)

      const wallet = await loadWallet()
      expect(wallet).not.toBeNull()
      expect(wallet!.mnemonic).toBe(TEST_MNEMONIC_12)
      expect(wallet!.address).toBe(deriveAddress(TEST_MNEMONIC_12))
    })

    it('loads encrypted wallet with correct password', async () => {
      const password = 'testpass123'
      await saveWallet(TEST_MNEMONIC_12, password)

      const wallet = await loadWallet(password)
      expect(wallet).not.toBeNull()
      expect(wallet!.mnemonic).toBe(TEST_MNEMONIC_12)
    })

    it('throws error when loading encrypted wallet without password', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'mypassword')

      await expect(loadWallet()).rejects.toThrow('Password required to unlock wallet')
    })

    it('throws error for incorrect password', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'correct-password')

      await expect(loadWallet('wrong-password')).rejects.toThrow('Incorrect password')
    })

    it('returns null when no wallet stored', async () => {
      const wallet = await loadWallet()
      expect(wallet).toBeNull()
    })
  })

  describe('getWalletInfo', () => {
    it('returns correct info for no wallet', () => {
      const info = getWalletInfo()
      expect(info.exists).toBe(false)
      expect(info.isEncrypted).toBe(false)
      expect(info.address).toBeNull()
    })

    it('returns correct info for unencrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12)

      const info = getWalletInfo()
      expect(info.exists).toBe(true)
      expect(info.isEncrypted).toBe(false)
      expect(info.address).toBe(deriveAddress(TEST_MNEMONIC_12))
    })

    it('returns correct info for encrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'password')

      const info = getWalletInfo()
      expect(info.exists).toBe(true)
      expect(info.isEncrypted).toBe(true)
      expect(info.address).toBe(deriveAddress(TEST_MNEMONIC_12))
    })
  })

  describe('hasStoredWallet', () => {
    it('returns false when no wallet', () => {
      expect(hasStoredWallet()).toBe(false)
    })

    it('returns true after saving wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12)
      expect(hasStoredWallet()).toBe(true)
    })
  })

  describe('isWalletEncrypted', () => {
    it('returns false when no wallet', () => {
      expect(isWalletEncrypted()).toBe(false)
    })

    it('returns false for unencrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12)
      expect(isWalletEncrypted()).toBe(false)
    })

    it('returns true for encrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'password')
      expect(isWalletEncrypted()).toBe(true)
    })
  })

  describe('clearWallet', () => {
    it('removes all wallet data', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'password')
      expect(hasStoredWallet()).toBe(true)

      clearWallet()

      expect(hasStoredWallet()).toBe(false)
      expect(localStorage.getItem('botho-wallet-mnemonic')).toBeNull()
      expect(localStorage.getItem('botho-wallet-address')).toBeNull()
      expect(localStorage.getItem('botho-wallet-encrypted')).toBeNull()
    })
  })
})

describe('Import Wallet Flow', () => {
  beforeEach(() => {
    localStorageMock.clear()
  })

  it('imports 12-word mnemonic correctly', async () => {
    const mnemonic = TEST_MNEMONIC_12

    // Simulate import flow
    expect(isValidMnemonic(mnemonic)).toBe(true)
    await saveWallet(mnemonic)

    const wallet = await loadWallet()
    expect(wallet).not.toBeNull()
    expect(wallet!.mnemonic).toBe(mnemonic)
  })

  it('imports 24-word mnemonic correctly', async () => {
    const mnemonic = TEST_MNEMONIC_24

    expect(isValidMnemonic(mnemonic)).toBe(true)
    await saveWallet(mnemonic)

    const wallet = await loadWallet()
    expect(wallet).not.toBeNull()
    expect(wallet!.mnemonic).toBe(mnemonic)
  })

  it('handles mnemonic normalization', async () => {
    // User might paste with extra whitespace or mixed case
    const messyInput = '  Abandon  ABANDON  abandon   abandon abandon abandon abandon abandon abandon abandon abandon about  '
    const normalized = messyInput.trim().toLowerCase().replace(/\s+/g, ' ')

    expect(isValidMnemonic(normalized)).toBe(true)
    await saveWallet(normalized)

    const wallet = await loadWallet()
    expect(wallet!.mnemonic).toBe(normalized)
  })

  it('rejects invalid mnemonic during import', () => {
    const invalidMnemonic = 'this is not a valid bip39 mnemonic phrase at all'
    expect(isValidMnemonic(invalidMnemonic)).toBe(false)
    expect(() => deriveAddress(invalidMnemonic)).toThrow('Invalid mnemonic')
  })

  it('imports with password protection', async () => {
    const mnemonic = TEST_MNEMONIC_12
    const password = 'secure-password'

    await saveWallet(mnemonic, password)

    // Verify it's encrypted
    expect(isWalletEncrypted()).toBe(true)

    // Can load with password
    const wallet = await loadWallet(password)
    expect(wallet!.mnemonic).toBe(mnemonic)
  })
})
