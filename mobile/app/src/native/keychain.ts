/**
 * Secure Wallet Storage (iOS Keychain + Android Keystore)
 *
 * Provides secure storage for encrypted wallet data with biometric
 * protection. Uses `expo-secure-store` under the hood, which is a
 * cross-platform Expo module — NOT an iOS-only wrapper:
 *
 * - iOS: backed by the iOS Keychain (Security.framework), with
 *   `SecAccessControl` biometry flags when `requireAuthentication` is set.
 * - Android: backed by the Android Keystore + `EncryptedSharedPreferences`,
 *   with `BiometricPrompt` gating when `requireAuthentication` is set (the
 *   Keystore key is created with `setUserAuthenticationRequired(true)`).
 *
 * The same exported surface (`saveEncryptedWallet`, `loadEncryptedWallet`,
 * etc.) therefore works on both platforms without per-platform branching in
 * the callers (e.g. `walletStore.ts`). See the `SECURE_OPTIONS` doc comment
 * below for which options are cross-platform vs. iOS-only.
 */

import * as SecureStore from "expo-secure-store";
import * as LocalAuthentication from "expo-local-authentication";
import type { ManagedNode } from "../config/nodes";

/** Keychain key for encrypted wallet data */
const WALLET_KEY = "botho_encrypted_wallet";

/** Keychain key for sync height */
const SYNC_HEIGHT_KEY = "botho_sync_height";

/** Keychain key for node URL preference (the active/selected node). */
const NODE_URL_KEY = "botho_node_url";

/** Keychain key for the user-managed list of trusted nodes. */
const NODE_LIST_KEY = "botho_node_list";

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
 * Secure-store options for the encrypted wallet blob.
 *
 * Per-platform behaviour of each field (important: some fields are silently
 * ignored on Android, so the "maximum security" posture is NOT identical
 * across platforms and must not be assumed to be):
 *
 * - `keychainAccessible` (**iOS-only**): `WHEN_UNLOCKED_THIS_DEVICE_ONLY` maps
 *   to `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, which keeps the item out
 *   of iCloud Keychain backup. `expo-secure-store` **ignores this field on
 *   Android** — there is no Android accessibility constant. Android's own
 *   equivalent (excluding Keystore-backed data from Auto Backup) is applied by
 *   the platform by default, so the security intent still holds on Android,
 *   just via a different mechanism, not via this option.
 * - `requireAuthentication` (**cross-platform**): on iOS this sets
 *   `SecAccessControl` biometry flags; on Android it routes the read/write
 *   through `BiometricPrompt` and creates the Keystore key with
 *   `setUserAuthenticationRequired(true)`. NOTE: on Android this **requires at
 *   least one biometric or device credential to be enrolled** — otherwise
 *   `expo-secure-store` throws at write time. See `SECURE_STORE_UNAVAILABLE`
 *   below and `saveEncryptedWallet` / `loadEncryptedWallet` for how that is
 *   handled.
 * - `authenticationPrompt` (**cross-platform**): the message shown in the
 *   biometric/credential prompt on both platforms.
 *
 * Whether the Keystore key material lands in hardware-backed StrongBox / TEE
 * vs. a software keystore on Android is device-dependent and cannot be forced
 * from JS (there is no public `expo-secure-store` API for it). This is
 * documented in `mobile/README.md` and `mobile/FRAMEWORK_DECISION.md` rather
 * than enforced in code.
 */
const SECURE_OPTIONS: SecureStore.SecureStoreOptions = {
  keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY,
  requireAuthentication: true,
  authenticationPrompt: "Authenticate to access your Botho wallet",
};

/**
 * Thrown when the secure store cannot satisfy the `requireAuthentication`
 * contract because the device has no biometric or device-credential enrolled.
 *
 * This is most common on Android (many devices ship with "Swipe"/"None" screen
 * lock), but can also occur on a fresh iOS simulator/device with no Face ID /
 * Touch ID / passcode configured. Callers should surface an actionable message
 * asking the user to set up a screen lock, rather than treating this as a
 * generic storage failure.
 */
export class SecureStoreUnavailableError extends Error {
  constructor(
    message = "Secure storage requires a screen lock (biometric or device passcode/PIN). Please enable a screen lock in your device settings and try again."
  ) {
    super(message);
    this.name = "SecureStoreUnavailableError";
  }
}

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
 * Whether the device can satisfy `requireAuthentication`-gated secure storage.
 *
 * `requireAuthentication: true` needs an enrolled authenticator:
 * - biometric (Face ID / Touch ID / fingerprint), OR
 * - a device credential (PIN / pattern / passcode).
 *
 * `isBiometricAvailable()` only covers the biometric case, so we additionally
 * treat `SECURITY_LEVEL >= SECRET` (a passcode/PIN is set) as sufficient — a
 * device with a PIN but no fingerprint can still back `requireAuthentication`
 * via device-credential fallback. This is what lets us give the user an
 * actionable "set up a screen lock" message *before* the write throws.
 */
export async function isSecureStorageAvailable(): Promise<boolean> {
  // A biometric enrollment is sufficient on both platforms.
  if (await isBiometricAvailable()) return true;

  // Otherwise, a device credential (PIN/pattern/passcode) is also sufficient
  // for `requireAuthentication` via device-credential fallback.
  const level = await LocalAuthentication.getEnrolledLevelAsync();
  return level !== LocalAuthentication.SecurityLevel.NONE;
}

/**
 * Fallback decision for `requireAuthentication`-gated storage on a device with
 * no enrolled authenticator.
 *
 * Approach chosen (per issue #791 acceptance criteria — option "a" + "b"
 * combined): we PROACTIVELY detect the missing-enrollment case via
 * `isSecureStorageAvailable()` and throw a typed, actionable
 * `SecureStoreUnavailableError` before ever calling `expo-secure-store`. We
 * ALSO wrap the underlying call so that if the platform throws the native
 * "no authentication enrolled" error anyway (races, platform quirks), it is
 * normalized to the same typed error instead of leaking as an uncaught
 * rejection. This keeps callers on a single, catchable failure mode on both
 * iOS and Android.
 *
 * Rationale for erroring rather than silently downgrading to unauthenticated
 * storage: the wallet blob is the most sensitive data in the app; storing it
 * without `requireAuthentication` would be a silent security regression. It is
 * better to require the user to set a screen lock than to weaken protection
 * without their knowledge.
 */
async function setAuthenticatedItem(
  key: string,
  value: string
): Promise<void> {
  if (!(await isSecureStorageAvailable())) {
    throw new SecureStoreUnavailableError();
  }
  try {
    await SecureStore.setItemAsync(key, value, SECURE_OPTIONS);
  } catch (error) {
    if (isNoAuthEnrolledError(error)) {
      throw new SecureStoreUnavailableError();
    }
    throw error;
  }
}

/**
 * Best-effort classifier for the native "no authentication enrolled" error.
 *
 * `expo-secure-store` surfaces this differently per platform/version, so we
 * match on the well-known message fragments rather than a stable error code.
 */
function isNoAuthEnrolledError(error: unknown): boolean {
  const message =
    error instanceof Error ? error.message : String(error ?? "");
  return (
    /authentication/i.test(message) &&
    /(not|no).*(enrolled|set up|available|configured)/i.test(message)
  );
}

/**
 * Save encrypted wallet to secure storage (iOS Keychain / Android Keystore).
 *
 * Requires biometric or device-credential authentication to write. Throws
 * {@link SecureStoreUnavailableError} if the device has no screen lock
 * configured (see the fallback decision on `setAuthenticatedItem`).
 */
export async function saveEncryptedWallet(
  encryptedData: string
): Promise<void> {
  const wallet: StoredWallet = {
    encryptedData,
    createdAt: Date.now(),
    lastUnlock: Date.now(),
  };

  await setAuthenticatedItem(WALLET_KEY, JSON.stringify(wallet));
}

/**
 * Load encrypted wallet from secure storage (iOS Keychain / Android Keystore).
 *
 * Requires biometric or device-credential authentication to read. Returns null
 * if no wallet is stored. Re-throws {@link SecureStoreUnavailableError} so the
 * caller can prompt the user to set up a screen lock (rather than silently
 * treating "no lock configured" as "no wallet"); all other errors are logged
 * and result in `null`.
 */
export async function loadEncryptedWallet(): Promise<StoredWallet | null> {
  try {
    const data = await SecureStore.getItemAsync(WALLET_KEY, SECURE_OPTIONS);
    if (!data) return null;

    const wallet: StoredWallet = JSON.parse(data);

    // Update last unlock time (best-effort; failure here must not lose the
    // successfully-read wallet).
    wallet.lastUnlock = Date.now();
    try {
      await setAuthenticatedItem(WALLET_KEY, JSON.stringify(wallet));
    } catch (writeError) {
      console.warn("Failed to update wallet unlock timestamp:", writeError);
    }

    return wallet;
  } catch (error) {
    if (error instanceof SecureStoreUnavailableError) {
      throw error;
    }
    if (isNoAuthEnrolledError(error)) {
      throw new SecureStoreUnavailableError();
    }
    console.error("Failed to load wallet from secure storage:", error);
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

/**
 * Save the user-managed list of trusted nodes.
 *
 * Stored as JSON in the secure store (no biometric prompt: node URLs/identity
 * are not secrets, but they belong in secure store alongside the rest of the
 * wallet preferences and out of any cloud backup).
 */
export async function saveNodeList(nodes: ManagedNode[]): Promise<void> {
  await SecureStore.setItemAsync(NODE_LIST_KEY, JSON.stringify(nodes));
}

/**
 * Load the user-managed list of trusted nodes.
 *
 * Returns `null` when no list has been persisted yet (first run), so callers
 * can fall back to the seed node list.
 */
export async function loadNodeList(): Promise<ManagedNode[] | null> {
  try {
    const data = await SecureStore.getItemAsync(NODE_LIST_KEY);
    if (!data) return null;
    const parsed: unknown = JSON.parse(data);
    if (!Array.isArray(parsed)) return null;
    return parsed as ManagedNode[];
  } catch (error) {
    console.error("Failed to load node list:", error);
    return null;
  }
}
