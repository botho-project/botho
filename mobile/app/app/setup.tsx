/**
 * Setup Screen
 *
 * First-run flow: generate a new wallet via the bridge (`generate_wallet`,
 * which returns the 24-word mnemonic and auto-unlocks), show the mnemonic for
 * backup, then continue into the wallet. Also offers importing an existing
 * recovery phrase (handled on the unlock screen).
 *
 * Non-custodial: the mnemonic is shown once for the user to back up. It is held
 * in native session memory by the bridge; the JS layer keeps it only long
 * enough to display it for backup.
 */

import { useState } from "react";
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ActivityIndicator,
  ScrollView,
  Alert,
} from "react-native";
import { useRouter } from "expo-router";
import { NativeWallet } from "../src/native/walletModule";
import { useWalletStore } from "../src/store/walletStore";

type Phase =
  | { state: "intro" }
  | { state: "generating" }
  | { state: "backup"; mnemonic: string }
  | { state: "error"; message: string };

export default function SetupScreen() {
  const router = useRouter();
  const { nodeUrl, checkSession } = useWalletStore();
  const [phase, setPhase] = useState<Phase>({ state: "intro" });

  const handleGenerate = async () => {
    setPhase({ state: "generating" });
    try {
      // Make sure the bridge is pointed at the selected node first.
      await NativeWallet.setNodeUrl(nodeUrl);
      const mnemonic = await NativeWallet.generateWallet();
      setPhase({ state: "backup", mnemonic });
    } catch (error) {
      setPhase({
        state: "error",
        message:
          error instanceof Error ? error.message : "Failed to create wallet",
      });
    }
  };

  const handleContinue = async () => {
    // generate_wallet auto-unlocks the bridge; sync our store to that session.
    await checkSession();
    router.replace("/");
  };

  const handleConfirmBackup = () => {
    Alert.alert(
      "Backed up your phrase?",
      "Make sure you've written down all 24 words. You'll need them to recover your wallet.",
      [
        { text: "Not yet", style: "cancel" },
        { text: "I've saved it", onPress: handleContinue },
      ]
    );
  };

  return (
    <ScrollView
      style={styles.container}
      contentContainerStyle={styles.content}
    >
      {phase.state === "intro" && (
        <>
          <Text style={styles.icon}>🪙</Text>
          <Text style={styles.title}>Welcome to Botho</Text>
          <Text style={styles.subtitle}>
            Create a new testnet wallet, or import an existing recovery phrase.
          </Text>

          <TouchableOpacity style={styles.button} onPress={handleGenerate}>
            <Text style={styles.buttonText}>Create New Wallet</Text>
          </TouchableOpacity>

          <TouchableOpacity
            style={styles.secondaryButton}
            onPress={() => router.push("/unlock")}
          >
            <Text style={styles.secondaryButtonText}>
              Import Recovery Phrase
            </Text>
          </TouchableOpacity>
        </>
      )}

      {phase.state === "generating" && (
        <View style={styles.centered}>
          <ActivityIndicator size="large" color="#00d9ff" />
          <Text style={styles.loadingText}>Creating your wallet…</Text>
        </View>
      )}

      {phase.state === "backup" && (
        <>
          <Text style={styles.title}>Back Up Your Phrase</Text>
          <Text style={styles.subtitle}>
            Write down these 24 words in order and keep them safe. Anyone with
            this phrase can access your funds.
          </Text>

          <View style={styles.mnemonicBox}>
            <Text style={styles.mnemonicText} selectable>
              {phase.mnemonic}
            </Text>
          </View>

          <TouchableOpacity
            style={styles.button}
            onPress={handleConfirmBackup}
          >
            <Text style={styles.buttonText}>Continue</Text>
          </TouchableOpacity>
        </>
      )}

      {phase.state === "error" && (
        <>
          <View style={styles.errorBox}>
            <Text style={styles.errorText}>{phase.message}</Text>
          </View>
          <TouchableOpacity
            style={styles.button}
            onPress={() => setPhase({ state: "intro" })}
          >
            <Text style={styles.buttonText}>Try Again</Text>
          </TouchableOpacity>
        </>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: "#1a1a2e",
  },
  content: {
    flexGrow: 1,
    padding: 24,
    justifyContent: "center",
  },
  centered: {
    alignItems: "center",
  },
  icon: {
    fontSize: 64,
    textAlign: "center",
    marginBottom: 16,
  },
  title: {
    color: "#fff",
    fontSize: 26,
    fontWeight: "700",
    textAlign: "center",
    marginBottom: 12,
  },
  subtitle: {
    color: "#888",
    fontSize: 15,
    textAlign: "center",
    marginBottom: 32,
    lineHeight: 22,
  },
  loadingText: {
    color: "#888",
    marginTop: 16,
  },
  mnemonicBox: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 20,
    marginBottom: 32,
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  mnemonicText: {
    color: "#00d9ff",
    fontSize: 16,
    lineHeight: 28,
    fontFamily: "Courier",
  },
  button: {
    backgroundColor: "#00d9ff",
    borderRadius: 12,
    padding: 16,
    alignItems: "center",
    marginBottom: 16,
  },
  buttonText: {
    color: "#1a1a2e",
    fontSize: 18,
    fontWeight: "700",
  },
  secondaryButton: {
    padding: 16,
    alignItems: "center",
  },
  secondaryButtonText: {
    color: "#00d9ff",
    fontSize: 16,
  },
  errorBox: {
    backgroundColor: "#3a0a0a",
    borderRadius: 8,
    padding: 16,
    marginBottom: 24,
    borderWidth: 1,
    borderColor: "#ff4444",
  },
  errorText: {
    color: "#ff6666",
    fontSize: 14,
    textAlign: "center",
  },
});
