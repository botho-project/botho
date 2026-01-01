/**
 * Home Screen
 *
 * Main wallet view showing balance and quick actions.
 * Redirects to setup/unlock if wallet is not available.
 */

import { useEffect } from "react";
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ActivityIndicator,
  RefreshControl,
  ScrollView,
} from "react-native";
import { useRouter } from "expo-router";
import { useWalletStore } from "../src/store/walletStore";
import { hasStoredWallet } from "../src/native/keychain";

export default function HomeScreen() {
  const router = useRouter();
  const {
    isUnlocked,
    isLoading,
    error,
    address,
    balance,
    refreshBalance,
    clearError,
  } = useWalletStore();

  // Check if wallet exists on mount
  useEffect(() => {
    const checkWallet = async () => {
      const hasWallet = await hasStoredWallet();

      if (!hasWallet) {
        // No wallet - go to setup
        router.replace("/setup");
      } else if (!isUnlocked) {
        // Has wallet but locked - go to unlock
        router.push("/unlock");
      }
    };

    checkWallet();
  }, [isUnlocked, router]);

  const handleRefresh = () => {
    clearError();
    refreshBalance();
  };

  if (!isUnlocked) {
    return (
      <View style={styles.container}>
        <ActivityIndicator size="large" color="#00d9ff" />
        <Text style={styles.loadingText}>Loading wallet...</Text>
      </View>
    );
  }

  return (
    <ScrollView
      style={styles.container}
      contentContainerStyle={styles.content}
      refreshControl={
        <RefreshControl
          refreshing={isLoading}
          onRefresh={handleRefresh}
          tintColor="#00d9ff"
        />
      }
    >
      {/* Balance Card */}
      <View style={styles.balanceCard}>
        <Text style={styles.balanceLabel}>Total Balance</Text>
        <Text style={styles.balanceAmount}>
          {balance?.formatted ?? "0.000000 BTH"}
        </Text>
        <Text style={styles.addressText}>
          {address?.display ?? "Loading..."}
        </Text>
      </View>

      {/* Error Banner */}
      {error && (
        <View style={styles.errorBanner}>
          <Text style={styles.errorText}>{error}</Text>
          <TouchableOpacity onPress={clearError}>
            <Text style={styles.errorDismiss}>Dismiss</Text>
          </TouchableOpacity>
        </View>
      )}

      {/* Quick Actions */}
      <View style={styles.actionsRow}>
        <TouchableOpacity
          style={styles.actionButton}
          onPress={() => router.push("/receive")}
        >
          <Text style={styles.actionIcon}>↓</Text>
          <Text style={styles.actionLabel}>Receive</Text>
        </TouchableOpacity>

        <TouchableOpacity
          style={styles.actionButton}
          onPress={() => router.push("/send")}
        >
          <Text style={styles.actionIcon}>↑</Text>
          <Text style={styles.actionLabel}>Send</Text>
        </TouchableOpacity>
      </View>

      {/* Recent Transactions */}
      <View style={styles.section}>
        <View style={styles.sectionHeader}>
          <Text style={styles.sectionTitle}>Recent Transactions</Text>
          <TouchableOpacity onPress={() => router.push("/transactions")}>
            <Text style={styles.seeAllLink}>See All</Text>
          </TouchableOpacity>
        </View>

        <View style={styles.emptyState}>
          <Text style={styles.emptyText}>No transactions yet</Text>
          <Text style={styles.emptySubtext}>
            Your transaction history will appear here
          </Text>
        </View>
      </View>

      {/* Sync Status */}
      <View style={styles.syncStatus}>
        <View style={styles.syncDot} />
        <Text style={styles.syncText}>
          Synced to block {balance?.syncHeight ?? 0}
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
    padding: 20,
  },
  loadingText: {
    color: "#888",
    marginTop: 16,
    fontSize: 16,
  },

  // Balance Card
  balanceCard: {
    backgroundColor: "#16213e",
    borderRadius: 16,
    padding: 24,
    alignItems: "center",
    marginBottom: 24,
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  balanceLabel: {
    color: "#888",
    fontSize: 14,
    marginBottom: 8,
  },
  balanceAmount: {
    color: "#fff",
    fontSize: 36,
    fontWeight: "700",
    marginBottom: 12,
  },
  addressText: {
    color: "#00d9ff",
    fontSize: 12,
    fontFamily: "Courier",
  },

  // Error Banner
  errorBanner: {
    backgroundColor: "#ff4444",
    borderRadius: 8,
    padding: 12,
    marginBottom: 16,
    flexDirection: "row",
    justifyContent: "space-between",
    alignItems: "center",
  },
  errorText: {
    color: "#fff",
    flex: 1,
  },
  errorDismiss: {
    color: "#fff",
    fontWeight: "600",
    marginLeft: 12,
  },

  // Actions
  actionsRow: {
    flexDirection: "row",
    justifyContent: "space-around",
    marginBottom: 32,
  },
  actionButton: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 20,
    alignItems: "center",
    width: "45%",
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  actionIcon: {
    fontSize: 24,
    color: "#00d9ff",
    marginBottom: 8,
  },
  actionLabel: {
    color: "#fff",
    fontSize: 16,
    fontWeight: "500",
  },

  // Section
  section: {
    marginBottom: 24,
  },
  sectionHeader: {
    flexDirection: "row",
    justifyContent: "space-between",
    alignItems: "center",
    marginBottom: 12,
  },
  sectionTitle: {
    color: "#fff",
    fontSize: 18,
    fontWeight: "600",
  },
  seeAllLink: {
    color: "#00d9ff",
    fontSize: 14,
  },

  // Empty State
  emptyState: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 32,
    alignItems: "center",
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  emptyText: {
    color: "#888",
    fontSize: 16,
    marginBottom: 8,
  },
  emptySubtext: {
    color: "#666",
    fontSize: 14,
    textAlign: "center",
  },

  // Sync Status
  syncStatus: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "center",
    paddingVertical: 16,
  },
  syncDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    backgroundColor: "#00ff88",
    marginRight: 8,
  },
  syncText: {
    color: "#888",
    fontSize: 12,
  },
});
