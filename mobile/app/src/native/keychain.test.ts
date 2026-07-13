/**
 * Tests for the secure wallet storage layer (iOS Keychain / Android Keystore).
 *
 * These are the first tests `keychain.ts` has had on either platform (issue
 * #791). They run headlessly against mocked `expo-secure-store` and
 * `expo-local-authentication` — no device or emulator required — so they can
 * live in a plain `pnpm test` step without any mobile CI infra.
 *
 * Coverage focus:
 * - each exported function calls through to the mocked native modules with the
 *   expected arguments (including the per-platform `SECURE_OPTIONS`);
 * - the "no authenticator enrolled" fallback path (common on Android devices
 *   with "Swipe"/"None" screen lock) surfaces a typed
 *   `SecureStoreUnavailableError` rather than an uncaught rejection or a silent
 *   downgrade to unauthenticated storage.
 */

import * as SecureStore from "expo-secure-store";
import * as LocalAuthentication from "expo-local-authentication";

import {
  saveEncryptedWallet,
  loadEncryptedWallet,
  hasStoredWallet,
  deleteWallet,
  saveSyncHeight,
  loadSyncHeight,
  saveNodeUrl,
  loadNodeUrl,
  saveNodeList,
  loadNodeList,
  isBiometricAvailable,
  isSecureStorageAvailable,
  getBiometricType,
  authenticateWithBiometrics,
  SecureStoreUnavailableError,
} from "./keychain";
import type { ManagedNode } from "../config/nodes";

// --- Mocks -----------------------------------------------------------------

jest.mock("expo-secure-store", () => ({
  // iOS keychain accessibility constant — a plain sentinel value here.
  WHEN_UNLOCKED_THIS_DEVICE_ONLY: "WHEN_UNLOCKED_THIS_DEVICE_ONLY",
  setItemAsync: jest.fn().mockResolvedValue(undefined),
  getItemAsync: jest.fn().mockResolvedValue(null),
  deleteItemAsync: jest.fn().mockResolvedValue(undefined),
}));

jest.mock("expo-local-authentication", () => ({
  AuthenticationType: {
    FINGERPRINT: 1,
    FACIAL_RECOGNITION: 2,
  },
  SecurityLevel: {
    NONE: 0,
    SECRET: 1,
    BIOMETRIC: 2,
    BIOMETRIC_STRONG: 3,
  },
  hasHardwareAsync: jest.fn().mockResolvedValue(true),
  isEnrolledAsync: jest.fn().mockResolvedValue(true),
  getEnrolledLevelAsync: jest.fn().mockResolvedValue(2),
  supportedAuthenticationTypesAsync: jest.fn().mockResolvedValue([]),
  authenticateAsync: jest.fn().mockResolvedValue({ success: true }),
}));

const mockSecureStore = SecureStore as jest.Mocked<typeof SecureStore>;
const mockAuth = LocalAuthentication as jest.Mocked<typeof LocalAuthentication>;

/** The exact options object keychain.ts uses for the authenticated blob. */
const EXPECTED_SECURE_OPTIONS = {
  keychainAccessible: "WHEN_UNLOCKED_THIS_DEVICE_ONLY",
  requireAuthentication: true,
  authenticationPrompt: "Authenticate to access your Botho wallet",
};

beforeEach(() => {
  jest.clearAllMocks();
  // Default: a healthy device with a biometric enrolled.
  mockAuth.hasHardwareAsync.mockResolvedValue(true);
  mockAuth.isEnrolledAsync.mockResolvedValue(true);
  mockAuth.getEnrolledLevelAsync.mockResolvedValue(
    LocalAuthentication.SecurityLevel.BIOMETRIC
  );
  mockSecureStore.getItemAsync.mockResolvedValue(null);
  mockSecureStore.setItemAsync.mockResolvedValue(undefined);
  mockSecureStore.deleteItemAsync.mockResolvedValue(undefined);
});

// --- Authenticated wallet blob ---------------------------------------------

describe("saveEncryptedWallet", () => {
  it("writes the wallet with the authenticated SECURE_OPTIONS", async () => {
    await saveEncryptedWallet("cipher-blob");

    expect(mockSecureStore.setItemAsync).toHaveBeenCalledTimes(1);
    const [key, value, options] = mockSecureStore.setItemAsync.mock.calls[0];
    expect(key).toBe("botho_encrypted_wallet");
    expect(options).toEqual(EXPECTED_SECURE_OPTIONS);

    const stored = JSON.parse(value as string);
    expect(stored.encryptedData).toBe("cipher-blob");
    expect(typeof stored.createdAt).toBe("number");
    expect(typeof stored.lastUnlock).toBe("number");
  });

  it("checks for an enrolled authenticator before writing", async () => {
    await saveEncryptedWallet("cipher-blob");
    // Either biometric enrollment or device-credential level is consulted.
    expect(
      mockAuth.isEnrolledAsync.mock.calls.length +
        mockAuth.getEnrolledLevelAsync.mock.calls.length
    ).toBeGreaterThan(0);
  });
});

describe("loadEncryptedWallet", () => {
  it("returns null when no wallet is stored", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce(null);
    await expect(loadEncryptedWallet()).resolves.toBeNull();
  });

  it("reads with SECURE_OPTIONS and refreshes the unlock timestamp", async () => {
    const stored = {
      encryptedData: "cipher-blob",
      createdAt: 1,
      lastUnlock: 1,
    };
    mockSecureStore.getItemAsync.mockResolvedValueOnce(JSON.stringify(stored));

    const wallet = await loadEncryptedWallet();

    expect(wallet?.encryptedData).toBe("cipher-blob");
    // Read used the authenticated options.
    expect(mockSecureStore.getItemAsync).toHaveBeenCalledWith(
      "botho_encrypted_wallet",
      EXPECTED_SECURE_OPTIONS
    );
    // Unlock timestamp write-back happened.
    expect(mockSecureStore.setItemAsync).toHaveBeenCalledTimes(1);
    expect(wallet?.lastUnlock).toBeGreaterThanOrEqual(stored.createdAt);
  });

  it("still returns the wallet when the timestamp write-back fails", async () => {
    const stored = {
      encryptedData: "cipher-blob",
      createdAt: 1,
      lastUnlock: 1,
    };
    mockSecureStore.getItemAsync.mockResolvedValueOnce(JSON.stringify(stored));
    mockSecureStore.setItemAsync.mockRejectedValueOnce(new Error("write boom"));

    const wallet = await loadEncryptedWallet();
    expect(wallet?.encryptedData).toBe("cipher-blob");
  });

  it("returns null on a generic read failure", async () => {
    mockSecureStore.getItemAsync.mockRejectedValueOnce(
      new Error("keystore corrupt")
    );
    await expect(loadEncryptedWallet()).resolves.toBeNull();
  });
});

// --- Fallback: no authenticator enrolled -----------------------------------

describe("no-authenticator-enrolled fallback", () => {
  function makeDeviceWithNoLock() {
    // Android "Swipe"/"None" security: no biometric, no device credential.
    mockAuth.hasHardwareAsync.mockResolvedValue(false);
    mockAuth.isEnrolledAsync.mockResolvedValue(false);
    mockAuth.getEnrolledLevelAsync.mockResolvedValue(
      LocalAuthentication.SecurityLevel.NONE
    );
  }

  it("isSecureStorageAvailable is false with no lock configured", async () => {
    makeDeviceWithNoLock();
    await expect(isSecureStorageAvailable()).resolves.toBe(false);
  });

  it("isSecureStorageAvailable is true with a PIN but no biometric", async () => {
    mockAuth.hasHardwareAsync.mockResolvedValue(false);
    mockAuth.isEnrolledAsync.mockResolvedValue(false);
    mockAuth.getEnrolledLevelAsync.mockResolvedValue(
      LocalAuthentication.SecurityLevel.SECRET
    );
    await expect(isSecureStorageAvailable()).resolves.toBe(true);
  });

  it("saveEncryptedWallet throws SecureStoreUnavailableError and does not write", async () => {
    makeDeviceWithNoLock();

    await expect(saveEncryptedWallet("cipher-blob")).rejects.toBeInstanceOf(
      SecureStoreUnavailableError
    );
    // Critically: we do NOT silently downgrade to an unauthenticated write.
    expect(mockSecureStore.setItemAsync).not.toHaveBeenCalled();
  });

  it("normalizes a native 'not enrolled' error to SecureStoreUnavailableError", async () => {
    // Device reports a lock (passes the proactive check) but the native call
    // still throws the platform 'no authentication enrolled' error.
    mockSecureStore.setItemAsync.mockRejectedValueOnce(
      new Error("Could not encrypt the value: no authentication is enrolled")
    );

    await expect(saveEncryptedWallet("cipher-blob")).rejects.toBeInstanceOf(
      SecureStoreUnavailableError
    );
  });

  it("loadEncryptedWallet re-throws SecureStoreUnavailableError from a native not-enrolled read", async () => {
    mockSecureStore.getItemAsync.mockRejectedValueOnce(
      new Error("No authentication is set up on this device")
    );

    await expect(loadEncryptedWallet()).rejects.toBeInstanceOf(
      SecureStoreUnavailableError
    );
  });
});

// --- Existence / deletion --------------------------------------------------

describe("hasStoredWallet", () => {
  it("does an unauthenticated existence read (no SECURE_OPTIONS)", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce("{}");
    await expect(hasStoredWallet()).resolves.toBe(true);
    expect(mockSecureStore.getItemAsync).toHaveBeenCalledWith(
      "botho_encrypted_wallet"
    );
  });

  it("returns false when nothing is stored", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce(null);
    await expect(hasStoredWallet()).resolves.toBe(false);
  });

  it("returns false (not throw) when the read errors", async () => {
    mockSecureStore.getItemAsync.mockRejectedValueOnce(new Error("boom"));
    await expect(hasStoredWallet()).resolves.toBe(false);
  });
});

describe("deleteWallet", () => {
  it("deletes the wallet and sync height keys", async () => {
    await deleteWallet();
    expect(mockSecureStore.deleteItemAsync).toHaveBeenCalledWith(
      "botho_encrypted_wallet"
    );
    expect(mockSecureStore.deleteItemAsync).toHaveBeenCalledWith(
      "botho_sync_height"
    );
  });
});

// --- Non-sensitive preferences (no auth gate) ------------------------------

describe("sync height", () => {
  it("saves the height as a string with no SECURE_OPTIONS", async () => {
    await saveSyncHeight(42);
    expect(mockSecureStore.setItemAsync).toHaveBeenCalledWith(
      "botho_sync_height",
      "42"
    );
  });

  it("loads and parses the height", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce("128");
    await expect(loadSyncHeight()).resolves.toBe(128);
  });

  it("defaults to 0 when unset", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce(null);
    await expect(loadSyncHeight()).resolves.toBe(0);
  });
});

describe("node URL", () => {
  it("saves the node URL", async () => {
    await saveNodeUrl("https://node.example.com");
    expect(mockSecureStore.setItemAsync).toHaveBeenCalledWith(
      "botho_node_url",
      "https://node.example.com"
    );
  });

  it("loads the node URL", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce("https://n.example");
    await expect(loadNodeUrl()).resolves.toBe("https://n.example");
  });
});

describe("node list", () => {
  const nodes = [
    { url: "https://a.example" },
    { url: "https://b.example" },
  ] as unknown as ManagedNode[];

  it("saves the node list as JSON", async () => {
    await saveNodeList(nodes);
    expect(mockSecureStore.setItemAsync).toHaveBeenCalledWith(
      "botho_node_list",
      JSON.stringify(nodes)
    );
  });

  it("loads and parses a stored node list", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce(JSON.stringify(nodes));
    await expect(loadNodeList()).resolves.toEqual(nodes);
  });

  it("returns null on first run (nothing stored)", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce(null);
    await expect(loadNodeList()).resolves.toBeNull();
  });

  it("returns null when the stored value is not an array", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce('{"not":"array"}');
    await expect(loadNodeList()).resolves.toBeNull();
  });

  it("returns null on malformed JSON", async () => {
    mockSecureStore.getItemAsync.mockResolvedValueOnce("{ broken");
    await expect(loadNodeList()).resolves.toBeNull();
  });
});

// --- Biometric helpers -----------------------------------------------------

describe("isBiometricAvailable", () => {
  it("is true when hardware exists and a biometric is enrolled", async () => {
    mockAuth.hasHardwareAsync.mockResolvedValue(true);
    mockAuth.isEnrolledAsync.mockResolvedValue(true);
    await expect(isBiometricAvailable()).resolves.toBe(true);
  });

  it("is false when there is no biometric hardware", async () => {
    mockAuth.hasHardwareAsync.mockResolvedValue(false);
    await expect(isBiometricAvailable()).resolves.toBe(false);
    expect(mockAuth.isEnrolledAsync).not.toHaveBeenCalled();
  });

  it("is false when hardware exists but nothing is enrolled", async () => {
    mockAuth.hasHardwareAsync.mockResolvedValue(true);
    mockAuth.isEnrolledAsync.mockResolvedValue(false);
    await expect(isBiometricAvailable()).resolves.toBe(false);
  });
});

describe("getBiometricType", () => {
  it("reports face when facial recognition is supported", async () => {
    mockAuth.supportedAuthenticationTypesAsync.mockResolvedValue([
      LocalAuthentication.AuthenticationType.FACIAL_RECOGNITION,
    ]);
    await expect(getBiometricType()).resolves.toBe("face");
  });

  it("reports fingerprint when fingerprint is supported", async () => {
    mockAuth.supportedAuthenticationTypesAsync.mockResolvedValue([
      LocalAuthentication.AuthenticationType.FINGERPRINT,
    ]);
    await expect(getBiometricType()).resolves.toBe("fingerprint");
  });

  it("reports null when nothing is supported", async () => {
    mockAuth.supportedAuthenticationTypesAsync.mockResolvedValue([]);
    await expect(getBiometricType()).resolves.toBeNull();
  });
});

describe("authenticateWithBiometrics", () => {
  it("passes the prompt through and returns success", async () => {
    mockAuth.authenticateAsync.mockResolvedValue({ success: true });
    await expect(authenticateWithBiometrics("Unlock")).resolves.toBe(true);
    expect(mockAuth.authenticateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ promptMessage: "Unlock" })
    );
  });

  it("returns false when authentication fails", async () => {
    mockAuth.authenticateAsync.mockResolvedValue({
      success: false,
      error: "user_cancel",
    });
    await expect(authenticateWithBiometrics()).resolves.toBe(false);
  });
});
