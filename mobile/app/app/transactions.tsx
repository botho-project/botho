/**
 * Transaction History Screen
 *
 * Displays paginated transaction history with pull-to-refresh.
 */

import { useEffect, useState, useCallback } from "react";
import {
  View,
  Text,
  StyleSheet,
  FlatList,
  TouchableOpacity,
  RefreshControl,
  ActivityIndicator,
} from "react-native";
import { useWalletStore } from "../src/store/walletStore";
import type { TransactionEntry } from "../src/types/wallet";

/** Format amount for display */
function formatAmount(amount: bigint): string {
  const isNegative = amount < 0n;
  const absAmount = isNegative ? -amount : amount;
  const bth = Number(absAmount) / 1_000_000_000_000;
  const sign = isNegative ? "-" : "+";
  return `${sign}${bth.toFixed(6)} BTH`;
}

/** Format timestamp for display */
function formatTimestamp(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const diff = now.getTime() - date.getTime();

  // Less than 24 hours - show relative time
  if (diff < 24 * 60 * 60 * 1000) {
    const hours = Math.floor(diff / (60 * 60 * 1000));
    if (hours < 1) {
      const minutes = Math.floor(diff / (60 * 1000));
      return `${minutes}m ago`;
    }
    return `${hours}h ago`;
  }

  // Less than 7 days - show day name
  if (diff < 7 * 24 * 60 * 60 * 1000) {
    const days = Math.floor(diff / (24 * 60 * 60 * 1000));
    return `${days}d ago`;
  }

  // Otherwise show date
  return date.toLocaleDateString();
}

/** Transaction list item */
function TransactionItem({ tx }: { tx: TransactionEntry }) {
  const isReceive = tx.direction === "receive";

  return (
    <TouchableOpacity style={styles.txItem}>
      <View style={styles.txIcon}>
        <Text style={[styles.txArrow, isReceive && styles.txArrowReceive]}>
          {isReceive ? "↓" : "↑"}
        </Text>
      </View>

      <View style={styles.txDetails}>
        <Text style={styles.txType}>
          {isReceive ? "Received" : "Sent"}
        </Text>
        <Text style={styles.txMeta}>
          Block {tx.blockHeight} • {formatTimestamp(tx.timestamp)}
        </Text>
        {tx.counterparty && (
          <Text style={styles.txCounterparty} numberOfLines={1}>
            {isReceive ? "From: " : "To: "}{tx.counterparty}
          </Text>
        )}
      </View>

      <Text style={[styles.txAmount, isReceive && styles.txAmountReceive]}>
        {formatAmount(tx.amount)}
      </Text>
    </TouchableOpacity>
  );
}

export default function TransactionsScreen() {
  const { transactions, isLoading, refreshTransactions } = useWalletStore();
  const [page, setPage] = useState(0);
  const [hasMore, setHasMore] = useState(true);

  const PAGE_SIZE = 20;

  // Initial load
  useEffect(() => {
    refreshTransactions(PAGE_SIZE);
  }, [refreshTransactions]);

  // Handle refresh
  const handleRefresh = useCallback(() => {
    setPage(0);
    setHasMore(true);
    refreshTransactions(PAGE_SIZE);
  }, [refreshTransactions]);

  // Load more
  const handleLoadMore = useCallback(() => {
    if (!hasMore || isLoading) return;

    const nextPage = page + 1;
    // TODO: Implement pagination in store
    // For now, just set hasMore to false
    setHasMore(false);
    setPage(nextPage);
  }, [page, hasMore, isLoading]);

  // Empty state
  if (!isLoading && transactions.length === 0) {
    return (
      <View style={styles.emptyContainer}>
        <Text style={styles.emptyIcon}>📭</Text>
        <Text style={styles.emptyTitle}>No Transactions</Text>
        <Text style={styles.emptySubtitle}>
          Your transaction history will appear here once you send or receive
          BTH.
        </Text>
      </View>
    );
  }

  return (
    <FlatList
      style={styles.container}
      data={transactions}
      keyExtractor={(tx) => tx.txHash}
      renderItem={({ item }) => <TransactionItem tx={item} />}
      refreshControl={
        <RefreshControl
          refreshing={isLoading && page === 0}
          onRefresh={handleRefresh}
          tintColor="#00d9ff"
        />
      }
      onEndReached={handleLoadMore}
      onEndReachedThreshold={0.5}
      ListFooterComponent={
        isLoading && page > 0 ? (
          <View style={styles.loadingMore}>
            <ActivityIndicator color="#00d9ff" />
          </View>
        ) : null
      }
      ItemSeparatorComponent={() => <View style={styles.separator} />}
      contentContainerStyle={styles.listContent}
    />
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: "#1a1a2e",
  },
  listContent: {
    padding: 16,
  },

  // Transaction Item
  txItem: {
    flexDirection: "row",
    alignItems: "center",
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
  },
  txIcon: {
    width: 40,
    height: 40,
    borderRadius: 20,
    backgroundColor: "#0f3460",
    alignItems: "center",
    justifyContent: "center",
    marginRight: 12,
  },
  txArrow: {
    fontSize: 18,
    color: "#ff4444",
  },
  txArrowReceive: {
    color: "#00ff88",
  },
  txDetails: {
    flex: 1,
  },
  txType: {
    color: "#fff",
    fontSize: 16,
    fontWeight: "500",
    marginBottom: 4,
  },
  txMeta: {
    color: "#888",
    fontSize: 12,
  },
  txCounterparty: {
    color: "#666",
    fontSize: 11,
    marginTop: 4,
    fontFamily: "Courier",
  },
  txAmount: {
    color: "#ff4444",
    fontSize: 14,
    fontWeight: "600",
    fontFamily: "Courier",
  },
  txAmountReceive: {
    color: "#00ff88",
  },

  // Separator
  separator: {
    height: 8,
  },

  // Loading More
  loadingMore: {
    paddingVertical: 20,
    alignItems: "center",
  },

  // Empty State
  emptyContainer: {
    flex: 1,
    backgroundColor: "#1a1a2e",
    alignItems: "center",
    justifyContent: "center",
    padding: 32,
  },
  emptyIcon: {
    fontSize: 48,
    marginBottom: 16,
  },
  emptyTitle: {
    color: "#fff",
    fontSize: 20,
    fontWeight: "600",
    marginBottom: 8,
  },
  emptySubtitle: {
    color: "#888",
    fontSize: 14,
    textAlign: "center",
    lineHeight: 20,
  },
});
