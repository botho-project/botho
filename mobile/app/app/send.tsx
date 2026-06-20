/**
 * Send Screen
 *
 * Builds + submits a transfer via the bridge's `send_transaction`. Takes a
 * recipient address (testnet `tbotho://1/...` form) and a BTH amount, converts
 * the amount to picocredits, and surfaces the resulting tx hash or a clear
 * error (insufficient funds / invalid address / network).
 */

import { useState } from "react";
import {
  View,
  Text,
  TextInput,
  StyleSheet,
  TouchableOpacity,
  ActivityIndicator,
  KeyboardAvoidingView,
  Platform,
  ScrollView,
} from "react-native";
import { useRouter } from "expo-router";
import { useWalletStore } from "../src/store/walletStore";

/** 1 BTH = 1e12 picocredits. */
const PICOCREDITS_PER_BTH = 1_000_000_000_000n;

/**
 * Parse a decimal BTH string into picocredits (bigint). Returns null if the
 * input is not a valid positive amount with at most 12 decimal places.
 */
function parseBthToPicocredits(input: string): bigint | null {
  const trimmed = input.trim();
  if (!/^\d*\.?\d*$/.test(trimmed) || trimmed === "" || trimmed === ".") {
    return null;
  }
  const [wholePart, fracPartRaw = ""] = trimmed.split(".");
  if (fracPartRaw.length > 12) return null;
  const fracPart = fracPartRaw.padEnd(12, "0");
  const whole = BigInt(wholePart || "0");
  const frac = BigInt(fracPart || "0");
  const total = whole * PICOCREDITS_PER_BTH + frac;
  if (total <= 0n) return null;
  return total;
}

type SendPhase =
  | { state: "form" }
  | { state: "sending" }
  | { state: "sent"; txHash: string }
  | { state: "error"; message: string };

export default function SendScreen() {
  const router = useRouter();
  const { balance, send } = useWalletStore();

  const [recipient, setRecipient] = useState("");
  const [amount, setAmount] = useState("");
  const [phase, setPhase] = useState<SendPhase>({ state: "form" });

  const parsedAmount = parseBthToPicocredits(amount);
  const recipientValid = recipient.trim().length > 0;
  const amountValid = parsedAmount != null;
  const insufficient =
    parsedAmount != null &&
    balance != null &&
    parsedAmount > balance.picocredits;
  const canSubmit =
    recipientValid &&
    amountValid &&
    !insufficient &&
    phase.state !== "sending";

  const handleSend = async () => {
    if (!recipientValid) {
      setPhase({ state: "error", message: "Enter a recipient address" });
      return;
    }
    if (parsedAmount == null) {
      setPhase({ state: "error", message: "Enter a valid amount" });
      return;
    }
    if (insufficient) {
      setPhase({ state: "error", message: "Insufficient balance" });
      return;
    }

    setPhase({ state: "sending" });
    try {
      const txHash = await send(recipient.trim(), parsedAmount);
      setPhase({ state: "sent", txHash });
    } catch (error) {
      setPhase({
        state: "error",
        message: error instanceof Error ? error.message : "Send failed",
      });
    }
  };

  if (phase.state === "sent") {
    return (
      <View style={styles.container}>
        <View style={styles.successContainer}>
          <Text style={styles.successIcon}>✅</Text>
          <Text style={styles.successTitle}>Transaction Sent</Text>
          <Text style={styles.successLabel}>Transaction hash</Text>
          <Text style={styles.txHash} selectable>
            {phase.txHash}
          </Text>
          <TouchableOpacity
            style={styles.button}
            onPress={() => router.back()}
          >
            <Text style={styles.buttonText}>Done</Text>
          </TouchableOpacity>
        </View>
      </View>
    );
  }

  return (
    <KeyboardAvoidingView
      style={styles.container}
      behavior={Platform.OS === "ios" ? "padding" : "height"}
    >
      <ScrollView contentContainerStyle={styles.content}>
        <View style={styles.balanceBox}>
          <Text style={styles.balanceLabel}>Available</Text>
          <Text style={styles.balanceText}>
            {balance?.formatted ?? "0.000000 BTH"}
          </Text>
        </View>

        <Text style={styles.inputLabel}>Recipient address</Text>
        <TextInput
          style={styles.input}
          value={recipient}
          onChangeText={(v) => {
            setRecipient(v);
            if (phase.state === "error") setPhase({ state: "form" });
          }}
          placeholder="tbotho://1/…"
          placeholderTextColor="#666"
          autoCapitalize="none"
          autoCorrect={false}
        />

        <Text style={styles.inputLabel}>Amount (BTH)</Text>
        <TextInput
          style={styles.input}
          value={amount}
          onChangeText={(v) => {
            setAmount(v);
            if (phase.state === "error") setPhase({ state: "form" });
          }}
          placeholder="0.000000"
          placeholderTextColor="#666"
          keyboardType="decimal-pad"
        />

        {amount.length > 0 && !amountValid && (
          <Text style={styles.fieldHint}>Enter a valid positive amount</Text>
        )}
        {insufficient && (
          <Text style={styles.fieldError}>Insufficient balance</Text>
        )}

        {phase.state === "error" && (
          <View style={styles.errorBox}>
            <Text style={styles.errorText}>{phase.message}</Text>
          </View>
        )}

        <TouchableOpacity
          style={[styles.button, !canSubmit && styles.buttonDisabled]}
          onPress={handleSend}
          disabled={!canSubmit}
        >
          {phase.state === "sending" ? (
            <ActivityIndicator color="#1a1a2e" />
          ) : (
            <Text style={styles.buttonText}>Send</Text>
          )}
        </TouchableOpacity>
      </ScrollView>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: "#1a1a2e",
  },
  content: {
    padding: 24,
  },
  balanceBox: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
    marginBottom: 24,
    alignItems: "center",
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  balanceLabel: {
    color: "#888",
    fontSize: 12,
    marginBottom: 6,
  },
  balanceText: {
    color: "#fff",
    fontSize: 22,
    fontWeight: "700",
  },
  inputLabel: {
    color: "#fff",
    fontSize: 14,
    fontWeight: "500",
    marginBottom: 8,
  },
  input: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    borderWidth: 1,
    borderColor: "#0f3460",
    padding: 16,
    color: "#fff",
    fontSize: 16,
    marginBottom: 16,
  },
  fieldHint: {
    color: "#888",
    fontSize: 12,
    marginTop: -8,
    marginBottom: 16,
  },
  fieldError: {
    color: "#ff6666",
    fontSize: 12,
    marginTop: -8,
    marginBottom: 16,
  },
  errorBox: {
    backgroundColor: "#3a0a0a",
    borderRadius: 8,
    padding: 14,
    marginBottom: 16,
    borderWidth: 1,
    borderColor: "#ff4444",
  },
  errorText: {
    color: "#ff6666",
    fontSize: 14,
  },
  button: {
    backgroundColor: "#00d9ff",
    borderRadius: 12,
    padding: 16,
    alignItems: "center",
    marginTop: 8,
  },
  buttonDisabled: {
    opacity: 0.5,
  },
  buttonText: {
    color: "#1a1a2e",
    fontSize: 18,
    fontWeight: "700",
  },

  // Success view
  successContainer: {
    flex: 1,
    padding: 24,
    justifyContent: "center",
    alignItems: "center",
  },
  successIcon: {
    fontSize: 64,
    marginBottom: 16,
  },
  successTitle: {
    color: "#fff",
    fontSize: 24,
    fontWeight: "700",
    marginBottom: 24,
  },
  successLabel: {
    color: "#888",
    fontSize: 12,
    marginBottom: 8,
  },
  txHash: {
    color: "#00d9ff",
    fontSize: 13,
    fontFamily: "Courier",
    textAlign: "center",
    marginBottom: 32,
    paddingHorizontal: 16,
  },
});
