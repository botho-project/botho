import { describe, it, expect, beforeEach } from 'vitest'
import {
  createMnemonic,
  createMnemonic12,
  isValidMnemonic,
  deriveDefaultSubaddressPublicKeys,
  formatAddress,
  encrypt,
  decrypt,
  saveWallet,
  loadWallet,
  getWalletInfo,
  hasStoredWallet,
  isWalletEncrypted,
  clearWallet,
  parseAddress,
  isValidAddress,
  shortenAddress,
  deriveKeypairs,
  loadWalletWithKey,
  isVaultBlob,
  needsRewrap,
  MIN_PASSWORD_LENGTH,
  passwordStrength,
  LEGACY_PBKDF2_ITERATIONS,
} from './index'

// Local test helper reproducing the wallet's per-mnemonic address determinism
// WITHOUT wasm. It packs the REAL classical default-subaddress keys plus
// deterministic placeholder post-quantum bytes of the correct v2 lengths; the
// real ML-KEM/ML-DSA derivation lives in @botho/wasm-signer (`deriveV2Address`).
// `parseAddress` only length-checks the PQ fields, so these round-trip.
function deriveAddress(
  mnemonic: string,
  network: 'mainnet' | 'testnet' = 'testnet',
): string {
  if (!isValidMnemonic(mnemonic)) throw new Error('Invalid mnemonic')
  const { viewPublic, spendPublic } = deriveDefaultSubaddressPublicKeys(mnemonic, 0)
  return formatAddress(
    viewPublic,
    spendPublic,
    new Uint8Array(1184),
    new Uint8Array(1952),
    network,
  )
}

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

    it('starts with tbotho://2/ prefix (testnet default)', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      expect(address.startsWith('tbotho://2/')).toBe(true)
    })

    it('uses botho://2/ prefix for mainnet', () => {
      const address = deriveAddress(TEST_MNEMONIC_12, 'mainnet')
      expect(address.startsWith('botho://2/')).toBe(true)
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
      // Verify it matches the tbotho://2/<base58> format
      expect(address).toMatch(/^tbotho:\/\/2\/[A-Za-z1-9]+$/)
    })

    it('produces base58-encoded public keys', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      // Extract the base58 part and verify it's valid
      const base58Part = address.slice('tbotho://2/'.length)
      // Base58 should only contain valid characters (no 0, O, I, l)
      expect(base58Part).toMatch(/^[A-HJ-NP-Za-km-z1-9]+$/)
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
      await saveWallet(TEST_MNEMONIC_12, undefined, deriveAddress(TEST_MNEMONIC_12))

      expect(localStorage.getItem('botho-wallet-mnemonic')).toBe(TEST_MNEMONIC_12)
      expect(localStorage.getItem('botho-wallet-encrypted')).toBe('false')
      expect(localStorage.getItem('botho-wallet-address')).toBeTruthy()
    })

    it('saves encrypted wallet with password', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'mypassword', deriveAddress(TEST_MNEMONIC_12))

      const stored = localStorage.getItem('botho-wallet-mnemonic')
      expect(stored).not.toBe(TEST_MNEMONIC_12) // Should be encrypted
      expect(localStorage.getItem('botho-wallet-encrypted')).toBe('true')
      expect(localStorage.getItem('botho-wallet-address')).toBeTruthy()
    })

    it('stores correct address', async () => {
      await saveWallet(TEST_MNEMONIC_12, undefined, deriveAddress(TEST_MNEMONIC_12))

      const storedAddress = localStorage.getItem('botho-wallet-address')
      const expectedAddress = deriveAddress(TEST_MNEMONIC_12)
      expect(storedAddress).toBe(expectedAddress)
    })
  })

  describe('loadWallet', () => {
    it('loads unencrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12, undefined, deriveAddress(TEST_MNEMONIC_12))

      const wallet = await loadWallet()
      expect(wallet).not.toBeNull()
      expect(wallet!.mnemonic).toBe(TEST_MNEMONIC_12)
      expect(wallet!.address).toBe(deriveAddress(TEST_MNEMONIC_12))
    })

    it('loads encrypted wallet with correct password', async () => {
      const password = 'testpass123'
      await saveWallet(TEST_MNEMONIC_12, password, deriveAddress(TEST_MNEMONIC_12))

      const wallet = await loadWallet(password)
      expect(wallet).not.toBeNull()
      expect(wallet!.mnemonic).toBe(TEST_MNEMONIC_12)
    })

    it('throws error when loading encrypted wallet without password', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'mypassword', deriveAddress(TEST_MNEMONIC_12))

      await expect(loadWallet()).rejects.toThrow('Password required to unlock wallet')
    })

    it('throws error for incorrect password', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'correct-password', deriveAddress(TEST_MNEMONIC_12))

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
      await saveWallet(TEST_MNEMONIC_12, undefined, deriveAddress(TEST_MNEMONIC_12))

      const info = getWalletInfo()
      expect(info.exists).toBe(true)
      expect(info.isEncrypted).toBe(false)
      expect(info.address).toBe(deriveAddress(TEST_MNEMONIC_12))
    })

    it('returns correct info for encrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'password', deriveAddress(TEST_MNEMONIC_12))

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
      await saveWallet(TEST_MNEMONIC_12, undefined, deriveAddress(TEST_MNEMONIC_12))
      expect(hasStoredWallet()).toBe(true)
    })
  })

  describe('isWalletEncrypted', () => {
    it('returns false when no wallet', () => {
      expect(isWalletEncrypted()).toBe(false)
    })

    it('returns false for unencrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12, undefined, deriveAddress(TEST_MNEMONIC_12))
      expect(isWalletEncrypted()).toBe(false)
    })

    it('returns true for encrypted wallet', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'password', deriveAddress(TEST_MNEMONIC_12))
      expect(isWalletEncrypted()).toBe(true)
    })
  })

  describe('clearWallet', () => {
    it('removes all wallet data', async () => {
      await saveWallet(TEST_MNEMONIC_12, 'password', deriveAddress(TEST_MNEMONIC_12))
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
    await saveWallet(mnemonic, undefined, deriveAddress(mnemonic))

    const wallet = await loadWallet()
    expect(wallet).not.toBeNull()
    expect(wallet!.mnemonic).toBe(mnemonic)
  })

  it('imports 24-word mnemonic correctly', async () => {
    const mnemonic = TEST_MNEMONIC_24

    expect(isValidMnemonic(mnemonic)).toBe(true)
    await saveWallet(mnemonic, undefined, deriveAddress(mnemonic))

    const wallet = await loadWallet()
    expect(wallet).not.toBeNull()
    expect(wallet!.mnemonic).toBe(mnemonic)
  })

  it('handles mnemonic normalization', async () => {
    // User might paste with extra whitespace or mixed case
    const messyInput = '  Abandon  ABANDON  abandon   abandon abandon abandon abandon abandon abandon abandon abandon about  '
    const normalized = messyInput.trim().toLowerCase().replace(/\s+/g, ' ')

    expect(isValidMnemonic(normalized)).toBe(true)
    await saveWallet(normalized, undefined, deriveAddress(normalized))

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

    await saveWallet(mnemonic, password, deriveAddress(mnemonic))

    // Verify it's encrypted
    expect(isWalletEncrypted()).toBe(true)

    // Can load with password
    const wallet = await loadWallet(password)
    expect(wallet!.mnemonic).toBe(mnemonic)
  })
})

describe('Address Utilities', () => {
  describe('parseAddress', () => {
    it('parses testnet address correctly', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      const parsed = parseAddress(address)

      expect(parsed.network).toBe('testnet')
      expect(parsed.viewPublic).toHaveLength(32)
      expect(parsed.spendPublic).toHaveLength(32)
    })

    it('parses mainnet address correctly', () => {
      const address = deriveAddress(TEST_MNEMONIC_12, 'mainnet')
      const parsed = parseAddress(address)

      expect(parsed.network).toBe('mainnet')
      expect(parsed.viewPublic).toHaveLength(32)
      expect(parsed.spendPublic).toHaveLength(32)
    })

    it('throws error for invalid prefix', () => {
      expect(() => parseAddress('invalid://1/abc123')).toThrow('Invalid address format')
    })

    it('handles whitespace in address', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      const parsed = parseAddress('  ' + address + '  ')

      expect(parsed.network).toBe('testnet')
    })
  })

  describe('isValidAddress', () => {
    it('returns true for valid testnet address', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      expect(isValidAddress(address)).toBe(true)
    })

    it('returns true for valid mainnet address', () => {
      const address = deriveAddress(TEST_MNEMONIC_12, 'mainnet')
      expect(isValidAddress(address)).toBe(true)
    })

    it('returns false for invalid address', () => {
      expect(isValidAddress('not-an-address')).toBe(false)
      expect(isValidAddress('')).toBe(false)
      expect(isValidAddress('tbotho://1/')).toBe(false)
    })
  })

  describe('shortenAddress', () => {
    it('shortens long addresses', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      const shortened = shortenAddress(address)

      expect(shortened).toContain('...')
      expect(shortened.length).toBeLessThan(address.length)
    })

    it('preserves short addresses', () => {
      const shortAddr = 'short'
      expect(shortenAddress(shortAddr)).toBe(shortAddr)
    })

    it('uses custom prefix and suffix lengths', () => {
      const address = deriveAddress(TEST_MNEMONIC_12)
      const shortened = shortenAddress(address, 20, 10)

      expect(shortened.startsWith(address.slice(0, 20))).toBe(true)
      expect(shortened.endsWith(address.slice(-10))).toBe(true)
    })
  })

  describe('deriveKeypairs', () => {
    it('derives deterministic keypairs', () => {
      const keypairs1 = deriveKeypairs(TEST_MNEMONIC_12)
      const keypairs2 = deriveKeypairs(TEST_MNEMONIC_12)

      expect(keypairs1.viewPublic).toEqual(keypairs2.viewPublic)
      expect(keypairs1.spendPublic).toEqual(keypairs2.spendPublic)
    })

    it('produces 32-byte keys', () => {
      const keypairs = deriveKeypairs(TEST_MNEMONIC_12)

      expect(keypairs.viewPrivate).toHaveLength(32)
      expect(keypairs.viewPublic).toHaveLength(32)
      expect(keypairs.spendPrivate).toHaveLength(32)
      expect(keypairs.spendPublic).toHaveLength(32)
    })

    it('produces different keypairs for different mnemonics', () => {
      const keypairs1 = deriveKeypairs(TEST_MNEMONIC_12)
      const keypairs2 = deriveKeypairs(TEST_MNEMONIC_24)

      expect(keypairs1.viewPublic).not.toEqual(keypairs2.viewPublic)
      expect(keypairs1.spendPublic).not.toEqual(keypairs2.spendPublic)
    })

    it('produces different view and spend keys', () => {
      const keypairs = deriveKeypairs(TEST_MNEMONIC_12)

      expect(keypairs.viewPublic).not.toEqual(keypairs.spendPublic)
      expect(keypairs.viewPrivate).not.toEqual(keypairs.spendPrivate)
    })
  })
})

// ============================================================================
// SECURITY (#475): encrypt-by-default policy, versioned KDF, migration
// ============================================================================

const STORAGE_KEY_MNEMONIC = 'botho-wallet-mnemonic'
const STORAGE_KEY_ADDRESS = 'botho-wallet-address'
const STORAGE_KEY_ENCRYPTED = 'botho-wallet-encrypted'

/** Re-create a LEGACY (pre-#475) 100k-iter, headerless encrypted blob. */
async function legacyEncrypt(plaintext: string, password: string): Promise<string> {
  const salt = crypto.getRandomValues(new Uint8Array(16))
  const iv = crypto.getRandomValues(new Uint8Array(12))
  const km = await crypto.subtle.importKey('raw', new TextEncoder().encode(password), 'PBKDF2', false, ['deriveKey'])
  const key = await crypto.subtle.deriveKey(
    { name: 'PBKDF2', salt, iterations: LEGACY_PBKDF2_ITERATIONS, hash: 'SHA-256' },
    km,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  )
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode(plaintext)))
  const out = new Uint8Array(28 + ct.length)
  out.set(salt, 0)
  out.set(iv, 16)
  out.set(ct, 28)
  return Array.from(out).map((b) => b.toString(16).padStart(2, '0')).join('')
}

describe('Password policy (#475)', () => {
  beforeEach(() => localStorage.clear())

  it('requires at least MIN_PASSWORD_LENGTH characters when saving with a password', async () => {
    expect(MIN_PASSWORD_LENGTH).toBeGreaterThanOrEqual(8)
    const short = 'a'.repeat(MIN_PASSWORD_LENGTH - 1)
    await expect(saveWallet(TEST_MNEMONIC_12, short, deriveAddress(TEST_MNEMONIC_12))).rejects.toThrow(/at least/i)
  })

  it('accepts a password at the minimum length', async () => {
    const ok = 'a'.repeat(MIN_PASSWORD_LENGTH)
    await expect(saveWallet(TEST_MNEMONIC_12, ok, deriveAddress(TEST_MNEMONIC_12))).resolves.toBeUndefined()
    expect(isWalletEncrypted()).toBe(true)
  })

  it('rates password strength', () => {
    expect(passwordStrength('short')).toBe('too-short')
    expect(passwordStrength('aaaaaaaa')).toBe('weak')
    expect(passwordStrength('Abcdefg1!xyz')).toBe('strong')
  })
})

describe('Encrypt-by-default + versioned KDF header (#475)', () => {
  beforeEach(() => localStorage.clear())

  it('writes a VERSIONED vault blob (not legacy) when a password is given', async () => {
    await saveWallet(TEST_MNEMONIC_12, 'a-good-password', deriveAddress(TEST_MNEMONIC_12))
    const blob = localStorage.getItem(STORAGE_KEY_MNEMONIC)!
    expect(blob).not.toContain(TEST_MNEMONIC_12) // encrypted
    expect(isVaultBlob(blob)).toBe(true) // versioned header present
    expect(needsRewrap(blob)).toBe(false) // already current
  })

  it('round-trips an encrypted wallet via the new format', async () => {
    const pw = 'a-good-password'
    await saveWallet(TEST_MNEMONIC_12, pw, deriveAddress(TEST_MNEMONIC_12))
    const stored = await loadWallet(pw)
    expect(stored!.mnemonic).toBe(TEST_MNEMONIC_12)
  })

  it('rejects the wrong password on unlock', async () => {
    await saveWallet(TEST_MNEMONIC_12, 'the-right-password', deriveAddress(TEST_MNEMONIC_12))
    await expect(loadWallet('the-wrong-password')).rejects.toThrow('Incorrect password')
  })
})

describe('Legacy wallet migration on unlock (#475)', () => {
  beforeEach(() => localStorage.clear())

  it('migrates a legacy 100k-iter wallet to the versioned format on unlock', async () => {
    const pw = 'legacy-but-long-enough'
    // Seed storage in the OLD format by hand.
    const legacyBlob = await legacyEncrypt(TEST_MNEMONIC_12, pw)
    localStorage.setItem(STORAGE_KEY_MNEMONIC, legacyBlob)
    localStorage.setItem(STORAGE_KEY_ENCRYPTED, 'true')
    localStorage.setItem(STORAGE_KEY_ADDRESS, deriveAddress(TEST_MNEMONIC_12))

    expect(isVaultBlob(legacyBlob)).toBe(false)
    expect(needsRewrap(legacyBlob)).toBe(true)

    // Unlock: must succeed AND re-wrap to the current format.
    const stored = await loadWallet(pw)
    expect(stored!.mnemonic).toBe(TEST_MNEMONIC_12)

    const upgraded = localStorage.getItem(STORAGE_KEY_MNEMONIC)!
    expect(upgraded).not.toBe(legacyBlob)
    expect(isVaultBlob(upgraded)).toBe(true)
    expect(needsRewrap(upgraded)).toBe(false)

    // The migrated wallet still unlocks with the same password.
    const reread = await loadWallet(pw)
    expect(reread!.mnemonic).toBe(TEST_MNEMONIC_12)
  })

  it('migrates a legacy PLAINTEXT wallet to encrypted on next save', async () => {
    // Old plaintext storage: mnemonic stored verbatim, encrypted flag false.
    localStorage.setItem(STORAGE_KEY_MNEMONIC, TEST_MNEMONIC_12)
    localStorage.setItem(STORAGE_KEY_ENCRYPTED, 'false')
    localStorage.setItem(STORAGE_KEY_ADDRESS, deriveAddress(TEST_MNEMONIC_12))

    // A plaintext wallet loads without a password (so the user is never locked out).
    const loaded = await loadWallet()
    expect(loaded!.mnemonic).toBe(TEST_MNEMONIC_12)
    expect(getWalletInfo().isEncrypted).toBe(false)

    // Re-save WITH a password (the encrypt-by-default UI path) upgrades it.
    await saveWallet(loaded!.mnemonic, 'now-encrypted-pass', deriveAddress(loaded!.mnemonic))
    const blob = localStorage.getItem(STORAGE_KEY_MNEMONIC)!
    expect(blob).not.toBe(TEST_MNEMONIC_12)
    expect(isVaultBlob(blob)).toBe(true)
    expect(getWalletInfo().isEncrypted).toBe(true)
    const reread = await loadWallet('now-encrypted-pass')
    expect(reread!.mnemonic).toBe(TEST_MNEMONIC_12)
  })

  it('loadWalletWithKey returns a usable session vault key after migration', async () => {
    const pw = 'session-key-password'
    const legacyBlob = await legacyEncrypt(TEST_MNEMONIC_12, pw)
    localStorage.setItem(STORAGE_KEY_MNEMONIC, legacyBlob)
    localStorage.setItem(STORAGE_KEY_ENCRYPTED, 'true')
    localStorage.setItem(STORAGE_KEY_ADDRESS, deriveAddress(TEST_MNEMONIC_12))

    const unlocked = await loadWalletWithKey(pw)
    expect(unlocked!.mnemonic).toBe(TEST_MNEMONIC_12)
    expect(unlocked!.vaultKey).not.toBeNull()

    // The session key can encrypt sibling data (#474/#476) and read it back.
    const sibling = await unlocked!.vaultKey!.encryptString('claim-link-secret')
    expect(await unlocked!.vaultKey!.decryptString(sibling)).toBe('claim-link-secret')
  })
})
