/**
 * iOS Keychain Integration
 *
 * Provides secure storage for encrypted wallet data using iOS Keychain
 * with biometric protection. Uses expo-secure-store under the hood with
 * additional configuration for maximum security.
 */

import * as SecureStore from "expo-secure-store";
import * as LocalAuthentication from "expo-local-authentication";

/** Keychain key for encrypted wallet data */
const WALLET_KEY = "botho_encrypted_wallet";

/** Keychain key for sync height */
const SYNC_HEIGHT_KEY = "botho_sync_height";

/** Keychain key for node URL preference */
const NODE_URL_KEY = "botho_node_url";

/** Stored wallet data structure */
export interface StoredWallet {
  /** Encrypted wallet data (from Rust) */
  encryptedData: string;
  /** Creation timestamp */
  createdAt: number;
  /** Last unlock timestamp */
  lastUnlock: number;
}

/**
 * Keychain options for maximum security
 *
 * - kSecAttrAccessibleWhenUnlockedThisDeviceOnly: No iCloud backup
 * - requireAuthentication: Require biometric/passcode
 */
const SECURE_OPTIONS: SecureStore.SecureStoreOptions = {
  keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY,
  requireAuthentication: true,
  authenticationPrompt: "Authenticate to access your Botho wallet",
};

/**
 * Check if biometric authentication is available
 */
export async function isBiometricAvailable(): Promise<boolean> {
  const compatible = await LocalAuthentication.hasHardwareAsync();
  if (!compatible) return false;

  const enrolled = await LocalAuthentication.isEnrolledAsync();
  return enrolled;
}

/**
 * Get available biometric type
 */
export async function getBiometricType(): Promise<"face" | "fingerprint" | null> {
  const types = await LocalAuthentication.supportedAuthenticationTypesAsync();

  if (types.includes(LocalAuthentication.AuthenticationType.FACIAL_RECOGNITION)) {
    return "face";
  }
  if (types.includes(LocalAuthentication.AuthenticationType.FINGERPRINT)) {
    return "fingerprint";
  }
  return null;
}

/**
 * Authenticate with biometrics
 */
export async function authenticateWithBiometrics(
  reason = "Authenticate to access your wallet"
): Promise<boolean> {
  const result = await LocalAuthentication.authenticateAsync({
    promptMessage: reason,
    fallbackLabel: "Use passcode",
    disableDeviceFallback: false,
  });

  return result.success;
}

/**
 * Save encrypted wallet to Keychain
 *
 * Requires biometric authentication to write.
 */
export async function saveEncryptedWallet(
  encryptedData: string
): Promise<void> {
  const wallet: StoredWallet = {
    encryptedData,
    createdAt: Date.now(),
    lastUnlock: Date.now(),
  };

  await SecureStore.setItemAsync(
    WALLET_KEY,
    JSON.stringify(wallet),
    SECURE_OPTIONS
  );
}

/**
 * Load encrypted wallet from Keychain
 *
 * Requires biometric authentication to read.
 * Returns null if no wallet is stored.
 */
export async function loadEncryptedWallet(): Promise<StoredWallet | null> {
  try {
    const data = await SecureStore.getItemAsync(WALLET_KEY, SECURE_OPTIONS);
    if (!data) return null;

    const wallet: StoredWallet = JSON.parse(data);

    // Update last unlock time
    wallet.lastUnlock = Date.now();
    await SecureStore.setItemAsync(
      WALLET_KEY,
      JSON.stringify(wallet),
      SECURE_OPTIONS
    );

    return wallet;
  } catch (error) {
    console.error("Failed to load wallet from Keychain:", error);
    return null;
  }
}

/**
 * Check if a wallet exists in Keychain
 *
 * Does NOT require biometric authentication.
 */
export async function hasStoredWallet(): Promise<boolean> {
  try {
    // Use non-authenticated read just to check existence
    const data = await SecureStore.getItemAsync(WALLET_KEY);
    return data !== null;
  } catch {
    return false;
  }
}

/**
 * Delete wallet from Keychain
 *
 * WARNING: This is irreversible. User must have their mnemonic backup.
 */
export async function deleteWallet(): Promise<void> {
  await SecureStore.deleteItemAsync(WALLET_KEY);
  await SecureStore.deleteItemAsync(SYNC_HEIGHT_KEY);
}

/**
 * Save sync height (non-sensitive, no biometric required)
 */
export async function saveSyncHeight(height: number): Promise<void> {
  await SecureStore.setItemAsync(SYNC_HEIGHT_KEY, height.toString());
}

/**
 * Load sync height
 */
export async function loadSyncHeight(): Promise<number> {
  const height = await SecureStore.getItemAsync(SYNC_HEIGHT_KEY);
  return height ? parseInt(height, 10) : 0;
}

/**
 * Save preferred node URL
 */
export async function saveNodeUrl(url: string): Promise<void> {
  await SecureStore.setItemAsync(NODE_URL_KEY, url);
}

/**
 * Load preferred node URL
 */
export async function loadNodeUrl(): Promise<string | null> {
  return SecureStore.getItemAsync(NODE_URL_KEY);
}
