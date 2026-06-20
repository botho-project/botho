/**
 * Receive Screen
 *
 * Shows the wallet's own address so others can send to it. The address is
 * rendered as selectable text (long-press to copy). A scannable QR code is a
 * trivial follow-up once a QR dependency is added (see note below) — kept out
 * here to avoid pulling a new native dependency into the demo build.
 */

import { useEffect } from "react";
import {
  View,
  Text,
  StyleSheet,
  ActivityIndicator,
  ScrollView,
} from "react-native";
import { useWalletStore } from "../src/store/walletStore";

export default function ReceiveScreen() {
  const { address, checkSession } = useWalletStore();

  // Make sure we have a fresh address/session when this screen opens.
  useEffect(() => {
    if (!address) {
      checkSession();
    }
  }, [address, checkSession]);

  if (!address) {
    return (
      <View style={styles.loadingContainer}>
        <ActivityIndicator size="large" color="#00d9ff" />
        <Text style={styles.loadingText}>Loading address…</Text>
      </View>
    );
  }

  return (
    <ScrollView
      style={styles.container}
      contentContainerStyle={styles.content}
    >
      <Text style={styles.title}>Your Address</Text>
      <Text style={styles.subtitle}>
        Share this address to receive testnet BTH.
      </Text>

      {/* Address placeholder block (stands in for a QR code). */}
      <View style={styles.qrPlaceholder}>
        <Text style={styles.qrPlaceholderText}>📥</Text>
        <Text style={styles.qrPlaceholderHint}>
          Address below — long-press to copy
        </Text>
      </View>

      <View style={styles.addressCard}>
        <Text style={styles.addressLabel}>Address</Text>
        <Text style={styles.addressText} selectable>
          {address.display}
        </Text>
      </View>

      <View style={styles.keysCard}>
        <Text style={styles.keysLabel}>View public key</Text>
        <Text style={styles.keysValue} selectable numberOfLines={1}>
          {address.viewPublicKey}
        </Text>
        <Text style={[styles.keysLabel, styles.keysLabelSpaced]}>
          Spend public key
        </Text>
        <Text style={styles.keysValue} selectable numberOfLines={1}>
          {address.spendPublicKey}
        </Text>
      </View>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: "#1a1a2e",
  },
  content: {
    padding: 24,
    alignItems: "center",
  },
  loadingContainer: {
    flex: 1,
    backgroundColor: "#1a1a2e",
    alignItems: "center",
    justifyContent: "center",
  },
  loadingText: {
    color: "#888",
    marginTop: 16,
  },
  title: {
    color: "#fff",
    fontSize: 24,
    fontWeight: "700",
    marginBottom: 8,
  },
  subtitle: {
    color: "#888",
    fontSize: 14,
    textAlign: "center",
    marginBottom: 24,
  },
  qrPlaceholder: {
    width: 200,
    height: 200,
    borderRadius: 16,
    backgroundColor: "#16213e",
    borderWidth: 1,
    borderColor: "#0f3460",
    alignItems: "center",
    justifyContent: "center",
    marginBottom: 24,
  },
  qrPlaceholderText: {
    fontSize: 56,
    marginBottom: 8,
  },
  qrPlaceholderHint: {
    color: "#666",
    fontSize: 12,
  },
  addressCard: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
    width: "100%",
    borderWidth: 1,
    borderColor: "#0f3460",
    marginBottom: 16,
  },
  addressLabel: {
    color: "#888",
    fontSize: 12,
    marginBottom: 8,
  },
  addressText: {
    color: "#00d9ff",
    fontSize: 13,
    fontFamily: "Courier",
  },
  keysCard: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
    width: "100%",
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  keysLabel: {
    color: "#888",
    fontSize: 12,
    marginBottom: 6,
  },
  keysLabelSpaced: {
    marginTop: 16,
  },
  keysValue: {
    color: "#aaa",
    fontSize: 11,
    fontFamily: "Courier",
  },
});
