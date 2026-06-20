/**
 * Node Picker Screen (pick 1 of 3 testnet nodes)
 *
 * Lets the user choose the trusted node the thin wallet uses as RPC ingress.
 * The selection is persisted (keychain) and pushed to the rust-bridge via
 * `setNodeUrl`; node health (height / sync / peers) is shown via
 * `get_node_status`.
 */

import { useEffect, useState } from "react";
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ScrollView,
  ActivityIndicator,
} from "react-native";
import { useRouter } from "expo-router";
import { useWalletStore } from "../src/store/walletStore";
import { TESTNET_NODES, type NodeOption } from "../src/config/nodes";
import { NativeWallet } from "../src/native/walletModule";
import type { NodeStatusInfo } from "../src/types/wallet";

/** Per-node health probe result. */
type Health =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "ok"; status: NodeStatusInfo }
  | { state: "error"; message: string };

export default function NodePickerScreen() {
  const router = useRouter();
  const { nodeUrl, setNodeUrl } = useWalletStore();
  const [health, setHealth] = useState<Record<string, Health>>({});
  const [applying, setApplying] = useState<string | null>(null);

  // Probe every node's health on mount so the user can compare.
  useEffect(() => {
    let cancelled = false;

    const probeAll = async () => {
      for (const node of TESTNET_NODES) {
        if (cancelled) return;
        setHealth((h) => ({ ...h, [node.id]: { state: "loading" } }));
        try {
          // Point the bridge at this node, then read its status.
          await NativeWallet.setNodeUrl(node.url);
          const status = await NativeWallet.getNodeStatus();
          if (cancelled) return;
          setHealth((h) => ({ ...h, [node.id]: { state: "ok", status } }));
        } catch (error) {
          if (cancelled) return;
          const message =
            error instanceof Error ? error.message : "Unreachable";
          setHealth((h) => ({
            ...h,
            [node.id]: { state: "error", message },
          }));
        }
      }
      // Restore the bridge to the currently-selected node after probing.
      if (!cancelled) {
        try {
          await NativeWallet.setNodeUrl(nodeUrl);
        } catch {
          // best-effort
        }
      }
    };

    probeAll();
    return () => {
      cancelled = true;
    };
    // Probe once on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleSelect = async (node: NodeOption) => {
    setApplying(node.id);
    try {
      await setNodeUrl(node.url);
      router.back();
    } finally {
      setApplying(null);
    }
  };

  return (
    <ScrollView
      style={styles.container}
      contentContainerStyle={styles.content}
    >
      <Text style={styles.intro}>
        Pick the trusted node your wallet connects to. The wallet scans and
        submits transactions through this node.
      </Text>

      {TESTNET_NODES.map((node) => {
        const selected = node.url === nodeUrl;
        const h = health[node.id] ?? { state: "idle" };
        return (
          <TouchableOpacity
            key={node.id}
            style={[styles.nodeCard, selected && styles.nodeCardSelected]}
            onPress={() => handleSelect(node)}
            disabled={applying != null}
          >
            <View style={styles.nodeHeader}>
              <Text style={styles.nodeLabel}>{node.label}</Text>
              {selected && <Text style={styles.selectedBadge}>SELECTED</Text>}
              {node.isFaucet && (
                <Text style={styles.faucetBadge}>FAUCET</Text>
              )}
            </View>

            <Text style={styles.nodeUrl}>{node.url}</Text>
            <Text style={styles.nodeDescription}>{node.description}</Text>

            <View style={styles.healthRow}>
              {h.state === "loading" && (
                <>
                  <ActivityIndicator size="small" color="#00d9ff" />
                  <Text style={styles.healthText}>Checking…</Text>
                </>
              )}
              {h.state === "ok" && (
                <>
                  <View style={[styles.healthDot, styles.healthDotOk]} />
                  <Text style={styles.healthText}>
                    Height {h.status.chainHeight} •{" "}
                    {h.status.syncStatus || "—"} • {h.status.peerCount} peers
                  </Text>
                </>
              )}
              {h.state === "error" && (
                <>
                  <View style={[styles.healthDot, styles.healthDotError]} />
                  <Text style={styles.healthTextError} numberOfLines={1}>
                    {h.message}
                  </Text>
                </>
              )}
            </View>

            {applying === node.id && (
              <ActivityIndicator
                style={styles.applying}
                size="small"
                color="#00d9ff"
              />
            )}
          </TouchableOpacity>
        );
      })}
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
  intro: {
    color: "#888",
    fontSize: 14,
    marginBottom: 20,
    lineHeight: 20,
  },
  nodeCard: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
    marginBottom: 16,
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  nodeCardSelected: {
    borderColor: "#00d9ff",
  },
  nodeHeader: {
    flexDirection: "row",
    alignItems: "center",
    marginBottom: 6,
  },
  nodeLabel: {
    color: "#fff",
    fontSize: 18,
    fontWeight: "600",
    flex: 1,
  },
  selectedBadge: {
    color: "#00d9ff",
    fontSize: 10,
    fontWeight: "700",
    marginLeft: 8,
  },
  faucetBadge: {
    color: "#00ff88",
    fontSize: 10,
    fontWeight: "700",
    marginLeft: 8,
  },
  nodeUrl: {
    color: "#00d9ff",
    fontSize: 13,
    fontFamily: "Courier",
    marginBottom: 4,
  },
  nodeDescription: {
    color: "#888",
    fontSize: 13,
    marginBottom: 12,
  },
  healthRow: {
    flexDirection: "row",
    alignItems: "center",
  },
  healthDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    marginRight: 8,
  },
  healthDotOk: {
    backgroundColor: "#00ff88",
  },
  healthDotError: {
    backgroundColor: "#ff4444",
  },
  healthText: {
    color: "#888",
    fontSize: 12,
    marginLeft: 6,
  },
  healthTextError: {
    color: "#ff6666",
    fontSize: 12,
    flex: 1,
  },
  applying: {
    marginTop: 12,
  },
});
