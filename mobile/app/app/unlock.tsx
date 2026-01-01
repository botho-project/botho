/**
 * Unlock Screen
 *
 * Authenticates user via biometrics or mnemonic to unlock the wallet.
 */

import { useState, useEffect } from "react";
import {
  View,
  Text,
  TextInput,
  TouchableOpacity,
  StyleSheet,
  ActivityIndicator,
  Alert,
  KeyboardAvoidingView,
  Platform,
} from "react-native";
import { useRouter } from "expo-router";
import { useWalletStore } from "../src/store/walletStore";
import {
  isBiometricAvailable,
  getBiometricType,
  authenticateWithBiometrics,
  loadEncryptedWallet,
} from "../src/native/keychain";

export default function UnlockScreen() {
  const router = useRouter();
  const { unlock, isLoading, error, clearError } = useWalletStore();

  const [biometricType, setBiometricType] = useState<"face" | "fingerprint" | null>(null);
  const [showMnemonicInput, setShowMnemonicInput] = useState(false);
  const [mnemonic, setMnemonic] = useState("");
  const [isAuthenticating, setIsAuthenticating] = useState(false);

  // Check biometric availability on mount
  useEffect(() => {
    const checkBiometrics = async () => {
      const available = await isBiometricAvailable();
      if (available) {
        const type = await getBiometricType();
        setBiometricType(type);
        // Auto-trigger biometric auth on mount
        handleBiometricAuth();
      } else {
        // No biometrics - show mnemonic input
        setShowMnemonicInput(true);
      }
    };

    checkBiometrics();
  }, []);

  // Handle biometric authentication
  const handleBiometricAuth = async () => {
    if (isAuthenticating) return;

    setIsAuthenticating(true);
    clearError();

    try {
      // Authenticate with biometrics
      const success = await authenticateWithBiometrics(
        "Authenticate to unlock your Botho wallet"
      );

      if (!success) {
        Alert.alert(
          "Authentication Failed",
          "Would you like to try again or enter your recovery phrase?",
          [
            { text: "Try Again", onPress: () => handleBiometricAuth() },
            { text: "Use Recovery Phrase", onPress: () => setShowMnemonicInput(true) },
          ]
        );
        return;
      }

      // Load encrypted wallet from Keychain
      const storedWallet = await loadEncryptedWallet();
      if (!storedWallet) {
        Alert.alert("Error", "Failed to load wallet data. Please restore with your recovery phrase.");
        setShowMnemonicInput(true);
        return;
      }

      // TODO: Decrypt wallet using native module and unlock
      // For now, redirect to home (simulated unlock)
      router.replace("/");
    } catch (err) {
      console.error("Biometric auth error:", err);
      Alert.alert("Error", "Authentication failed. Please try again.");
    } finally {
      setIsAuthenticating(false);
    }
  };

  // Handle mnemonic unlock
  const handleMnemonicUnlock = async () => {
    if (!mnemonic.trim()) {
      Alert.alert("Error", "Please enter your recovery phrase");
      return;
    }

    const words = mnemonic.trim().split(/\s+/);
    if (words.length !== 24) {
      Alert.alert(
        "Invalid Recovery Phrase",
        "Please enter all 24 words of your recovery phrase"
      );
      return;
    }

    try {
      await unlock(mnemonic.trim());
      router.replace("/");
    } catch (err) {
      // Error is handled by store
    }
  };

  const biometricLabel = biometricType === "face" ? "Face ID" : "Touch ID";
  const biometricIcon = biometricType === "face" ? "👤" : "👆";

  return (
    <KeyboardAvoidingView
      style={styles.container}
      behavior={Platform.OS === "ios" ? "padding" : "height"}
    >
      <View style={styles.content}>
        {/* Logo/Title */}
        <View style={styles.header}>
          <Text style={styles.logo}>🔐</Text>
          <Text style={styles.title}>Unlock Wallet</Text>
          <Text style={styles.subtitle}>
            Authenticate to access your Botho wallet
          </Text>
        </View>

        {/* Error Banner */}
        {error && (
          <View style={styles.errorBanner}>
            <Text style={styles.errorText}>{error}</Text>
          </View>
        )}

        {/* Biometric Button */}
        {biometricType && !showMnemonicInput && (
          <View style={styles.biometricSection}>
            <TouchableOpacity
              style={styles.biometricButton}
              onPress={handleBiometricAuth}
              disabled={isAuthenticating}
            >
              {isAuthenticating ? (
                <ActivityIndicator color="#fff" size="large" />
              ) : (
                <>
                  <Text style={styles.biometricIcon}>{biometricIcon}</Text>
                  <Text style={styles.biometricLabel}>
                    Unlock with {biometricLabel}
                  </Text>
                </>
              )}
            </TouchableOpacity>

            <TouchableOpacity
              style={styles.alternativeButton}
              onPress={() => setShowMnemonicInput(true)}
            >
              <Text style={styles.alternativeText}>
                Use recovery phrase instead
              </Text>
            </TouchableOpacity>
          </View>
        )}

        {/* Mnemonic Input */}
        {showMnemonicInput && (
          <View style={styles.mnemonicSection}>
            <Text style={styles.inputLabel}>Recovery Phrase</Text>
            <TextInput
              style={styles.mnemonicInput}
              value={mnemonic}
              onChangeText={setMnemonic}
              placeholder="Enter your 24-word recovery phrase"
              placeholderTextColor="#666"
              multiline
              numberOfLines={4}
              autoCapitalize="none"
              autoCorrect={false}
              secureTextEntry={false} // Show text for easier entry
              textAlignVertical="top"
            />

            <TouchableOpacity
              style={[styles.unlockButton, isLoading && styles.buttonDisabled]}
              onPress={handleMnemonicUnlock}
              disabled={isLoading}
            >
              {isLoading ? (
                <ActivityIndicator color="#fff" />
              ) : (
                <Text style={styles.unlockButtonText}>Unlock Wallet</Text>
              )}
            </TouchableOpacity>

            {biometricType && (
              <TouchableOpacity
                style={styles.alternativeButton}
                onPress={() => {
                  setShowMnemonicInput(false);
                  setMnemonic("");
                }}
              >
                <Text style={styles.alternativeText}>
                  Use {biometricLabel} instead
                </Text>
              </TouchableOpacity>
            )}
          </View>
        )}
      </View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: "#1a1a2e",
  },
  content: {
    flex: 1,
    padding: 24,
    justifyContent: "center",
  },

  // Header
  header: {
    alignItems: "center",
    marginBottom: 40,
  },
  logo: {
    fontSize: 64,
    marginBottom: 16,
  },
  title: {
    color: "#fff",
    fontSize: 28,
    fontWeight: "700",
    marginBottom: 8,
  },
  subtitle: {
    color: "#888",
    fontSize: 16,
    textAlign: "center",
  },

  // Error
  errorBanner: {
    backgroundColor: "#ff4444",
    borderRadius: 8,
    padding: 12,
    marginBottom: 24,
  },
  errorText: {
    color: "#fff",
    textAlign: "center",
  },

  // Biometric
  biometricSection: {
    alignItems: "center",
  },
  biometricButton: {
    backgroundColor: "#00d9ff",
    borderRadius: 16,
    padding: 24,
    alignItems: "center",
    width: "100%",
    marginBottom: 16,
  },
  biometricIcon: {
    fontSize: 48,
    marginBottom: 12,
  },
  biometricLabel: {
    color: "#fff",
    fontSize: 18,
    fontWeight: "600",
  },

  // Mnemonic Input
  mnemonicSection: {
    width: "100%",
  },
  inputLabel: {
    color: "#fff",
    fontSize: 14,
    fontWeight: "500",
    marginBottom: 8,
  },
  mnemonicInput: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    borderWidth: 1,
    borderColor: "#0f3460",
    padding: 16,
    color: "#fff",
    fontSize: 16,
    minHeight: 120,
    marginBottom: 16,
  },
  unlockButton: {
    backgroundColor: "#00d9ff",
    borderRadius: 12,
    padding: 16,
    alignItems: "center",
    marginBottom: 16,
  },
  buttonDisabled: {
    opacity: 0.6,
  },
  unlockButtonText: {
    color: "#fff",
    fontSize: 18,
    fontWeight: "600",
  },

  // Alternative
  alternativeButton: {
    padding: 12,
    alignItems: "center",
  },
  alternativeText: {
    color: "#00d9ff",
    fontSize: 14,
  },
});
