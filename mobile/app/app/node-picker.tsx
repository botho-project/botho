/**
 * Node Picker Screen — thin-client node selection / trust UX (epic #441 P3).
 *
 * The wallet is a thin client: it scans and submits transactions through a
 * single trusted node (its RPC ingress). This screen lets the user:
 *
 *   - Pick from a user-managed list of trusted nodes (seeded with the testnet
 *     defaults, no longer limited to 3 hardcoded entries).
 *   - Add a node by RPC URL after verifying its identity (`node_getIdentity`,
 *     #500): the app shows the node's peer ID / network / chain tip and refuses
 *     to trust a node on the wrong network or an incompatible protocol unless
 *     the user explicitly overrides a soft warning. Network mismatch is blocked.
 *   - Remove user-added nodes.
 *
 * Selection is persisted (secure store) and pushed to the rust-bridge via
 * `setNodeUrl`; per-node health (height / sync / peers) is shown via
 * `get_node_status` (same probe pattern as before).
 *
 * Privacy boundary (epic §5(c)): pointing at a node leaks which outputs are
 * the user's via the scan RPCs, so the screen surfaces a clear
 * "only point at nodes you trust" notice.
 */

import { useEffect, useState } from "react";
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ScrollView,
  ActivityIndicator,
  TextInput,
  Alert,
} from "react-native";
import { useRouter } from "expo-router";
import { useWalletStore, type VerifyNodeResult } from "../src/store/walletStore";
import type { ManagedNode } from "../src/config/nodes";
import { NativeWallet } from "../src/native/walletModule";
import { NodeIdentityError } from "../src/native/nodeIdentity";
import { EXPECTED_NETWORK } from "../src/config/network";
import type { NodeStatusInfo } from "../src/types/wallet";

/** Per-node health probe result. */
type Health =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "ok"; status: NodeStatusInfo }
  | { state: "error"; message: string };

/** Identity-verification flow state for the "add node" form. */
type Verify =
  | { state: "idle" }
  | { state: "verifying" }
  | { state: "error"; message: string }
  | { state: "confirm"; result: VerifyNodeResult };

export default function NodePickerScreen() {
  const router = useRouter();
  const {
    nodeUrl,
    nodes,
    setNodeUrl,
    verifyNode,
    addVerifiedNode,
    removeNode,
  } = useWalletStore();

  const [health, setHealth] = useState<Record<string, Health>>({});
  const [applying, setApplying] = useState<string | null>(null);

  // Add-by-URL form state.
  const [draftUrl, setDraftUrl] = useState("");
  const [draftLabel, setDraftLabel] = useState("");
  const [verify, setVerify] = useState<Verify>({ state: "idle" });

  // Probe every node's health on mount (and whenever the list grows).
  useEffect(() => {
    let cancelled = false;

    const probeAll = async () => {
      for (const node of nodes) {
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
    // Re-probe when the node list changes (e.g. after adding a node).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nodes.length]);

  const handleSelect = async (node: ManagedNode) => {
    setApplying(node.id);
    try {
      await setNodeUrl(node.url);
      router.back();
    } finally {
      setApplying(null);
    }
  };

  const handleRemove = (node: ManagedNode) => {
    Alert.alert(
      "Remove node",
      `Remove "${node.label}" from your trusted nodes?`,
      [
        { text: "Cancel", style: "cancel" },
        {
          text: "Remove",
          style: "destructive",
          onPress: () => {
            removeNode(node.id);
          },
        },
      ]
    );
  };

  // Step 1: verify the candidate node's identity before trusting it.
  const handleVerify = async () => {
    setVerify({ state: "verifying" });
    try {
      const result = await verifyNode(draftUrl);
      setVerify({ state: "confirm", result });
    } catch (error) {
      const message =
        error instanceof NodeIdentityError || error instanceof Error
          ? error.message
          : "Could not verify node.";
      setVerify({ state: "error", message });
    }
  };

  // Step 2: user has reviewed the identity and confirmed trust.
  const handleConfirmAdd = async (result: VerifyNodeResult) => {
    await addVerifiedNode(result, draftLabel);
    // Reset the form.
    setDraftUrl("");
    setDraftLabel("");
    setVerify({ state: "idle" });
  };

  const handleCancelVerify = () => {
    setVerify({ state: "idle" });
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

      {/* Privacy boundary notice (epic §5(c)). */}
      <View style={styles.privacyBanner}>
        <Text style={styles.privacyTitle}>Only connect to nodes you trust</Text>
        <Text style={styles.privacyText}>
          Your wallet asks this node which outputs belong to you, so a malicious
          node can learn your balance and activity. Add only nodes you operate
          or trust.
        </Text>
      </View>

      {nodes.map((node) => {
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
              {node.source === "user" && (
                <Text style={styles.userBadge}>ADDED</Text>
              )}
            </View>

            <Text style={styles.nodeUrl}>{node.url}</Text>
            <Text style={styles.nodeDescription}>{node.description}</Text>

            {node.verifiedIdentity && (
              <Text style={styles.identityLine} numberOfLines={1}>
                {node.verifiedIdentity.network} • peer{" "}
                {node.verifiedIdentity.peerId.slice(0, 12)}…
              </Text>
            )}

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

            {node.source === "user" && (
              <TouchableOpacity
                style={styles.removeButton}
                onPress={() => handleRemove(node)}
                disabled={applying != null}
              >
                <Text style={styles.removeButtonText}>Remove</Text>
              </TouchableOpacity>
            )}

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

      {/* Add-by-URL section. */}
      <View style={styles.addSection}>
        <Text style={styles.addTitle}>Add a node</Text>
        <Text style={styles.addHelp}>
          Enter a node&apos;s RPC URL. We&apos;ll verify its identity before you
          trust it.
        </Text>

        <TextInput
          style={styles.input}
          placeholder="https://node.example.com"
          placeholderTextColor="#555"
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="url"
          value={draftUrl}
          onChangeText={(t) => {
            setDraftUrl(t);
            if (verify.state === "error") setVerify({ state: "idle" });
          }}
          editable={verify.state !== "verifying"}
        />
        <TextInput
          style={styles.input}
          placeholder="Label (optional)"
          placeholderTextColor="#555"
          autoCapitalize="none"
          value={draftLabel}
          onChangeText={setDraftLabel}
          editable={verify.state !== "verifying"}
        />

        {verify.state === "error" && (
          <Text style={styles.verifyError}>{verify.message}</Text>
        )}

        <TouchableOpacity
          style={[
            styles.verifyButton,
            (draftUrl.trim() === "" || verify.state === "verifying") &&
              styles.verifyButtonDisabled,
          ]}
          onPress={handleVerify}
          disabled={draftUrl.trim() === "" || verify.state === "verifying"}
        >
          {verify.state === "verifying" ? (
            <ActivityIndicator size="small" color="#1a1a2e" />
          ) : (
            <Text style={styles.verifyButtonText}>Verify identity</Text>
          )}
        </TouchableOpacity>
      </View>

      {/* Identity confirmation card (shown after a successful verify). */}
      {verify.state === "confirm" && (
        <IdentityConfirm
          result={verify.result}
          onConfirm={() => handleConfirmAdd(verify.result)}
          onCancel={handleCancelVerify}
        />
      )}
    </ScrollView>
  );
}

/** Identity confirmation card: shows the verified identity + trust decision. */
function IdentityConfirm({
  result,
  onConfirm,
  onCancel,
}: {
  result: VerifyNodeResult;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { identity, networkMatches, protocolCompatible } = result;
  // Network mismatch is a hard block (wrong chain entirely). Protocol
  // incompatibility is a soft warning the user can override.
  const blocked = !networkMatches;

  return (
    <View style={styles.confirmCard}>
      <Text style={styles.confirmTitle}>Verify this node</Text>
      <Text style={styles.confirmUrl}>{result.url}</Text>

      <IdentityRow label="Network" value={identity.network} />
      <IdentityRow label="Peer ID" value={identity.peerId} mono />
      <IdentityRow label="Node ID" value={identity.nodeId} mono />
      <IdentityRow
        label="Protocol"
        value={`${identity.protocolVersion} (min ${identity.minProtocolVersion})`}
      />
      <IdentityRow
        label="Software"
        value={`${identity.nodeVersion} @ ${identity.gitCommit}`}
      />
      <IdentityRow
        label="Chain tip"
        value={`height ${identity.chainHeight}`}
      />

      {!networkMatches && (
        <Text style={styles.confirmDanger}>
          This node is on &quot;{identity.network || "an unknown network"}&quot;,
          but your wallet expects &quot;{EXPECTED_NETWORK}&quot;. Adding it is
          blocked to protect your funds.
        </Text>
      )}
      {networkMatches && !protocolCompatible && (
        <Text style={styles.confirmWarn}>
          This node&apos;s protocol version may be incompatible with this app.
          Some operations could fail.
        </Text>
      )}

      <View style={styles.confirmActions}>
        <TouchableOpacity style={styles.cancelButton} onPress={onCancel}>
          <Text style={styles.cancelButtonText}>Cancel</Text>
        </TouchableOpacity>
        <TouchableOpacity
          style={[styles.trustButton, blocked && styles.trustButtonDisabled]}
          onPress={onConfirm}
          disabled={blocked}
        >
          <Text style={styles.trustButtonText}>
            {protocolCompatible ? "Trust & add" : "Add anyway"}
          </Text>
        </TouchableOpacity>
      </View>
    </View>
  );
}

/** One labeled identity field row. */
function IdentityRow({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <View style={styles.idRow}>
      <Text style={styles.idLabel}>{label}</Text>
      <Text
        style={[styles.idValue, mono && styles.idValueMono]}
        numberOfLines={1}
      >
        {value || "—"}
      </Text>
    </View>
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
    marginBottom: 12,
    lineHeight: 20,
  },
  privacyBanner: {
    backgroundColor: "#2a1f3d",
    borderRadius: 10,
    borderWidth: 1,
    borderColor: "#5a3fb0",
    padding: 12,
    marginBottom: 20,
  },
  privacyTitle: {
    color: "#c9b6ff",
    fontSize: 13,
    fontWeight: "700",
    marginBottom: 4,
  },
  privacyText: {
    color: "#a99cc7",
    fontSize: 12,
    lineHeight: 17,
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
  userBadge: {
    color: "#ffb347",
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
    marginBottom: 8,
  },
  identityLine: {
    color: "#7a8aa0",
    fontSize: 11,
    fontFamily: "Courier",
    marginBottom: 8,
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
  removeButton: {
    marginTop: 12,
    alignSelf: "flex-start",
  },
  removeButtonText: {
    color: "#ff6666",
    fontSize: 13,
    fontWeight: "600",
  },
  applying: {
    marginTop: 12,
  },
  addSection: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
    marginTop: 8,
    marginBottom: 16,
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  addTitle: {
    color: "#fff",
    fontSize: 16,
    fontWeight: "600",
    marginBottom: 4,
  },
  addHelp: {
    color: "#888",
    fontSize: 12,
    lineHeight: 17,
    marginBottom: 12,
  },
  input: {
    backgroundColor: "#0f1830",
    borderRadius: 8,
    borderWidth: 1,
    borderColor: "#0f3460",
    color: "#fff",
    fontSize: 14,
    paddingHorizontal: 12,
    paddingVertical: 10,
    marginBottom: 10,
  },
  verifyError: {
    color: "#ff6666",
    fontSize: 12,
    marginBottom: 10,
  },
  verifyButton: {
    backgroundColor: "#00d9ff",
    borderRadius: 8,
    paddingVertical: 12,
    alignItems: "center",
  },
  verifyButtonDisabled: {
    opacity: 0.4,
  },
  verifyButtonText: {
    color: "#1a1a2e",
    fontSize: 15,
    fontWeight: "700",
  },
  confirmCard: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
    marginBottom: 24,
    borderWidth: 1,
    borderColor: "#00d9ff",
  },
  confirmTitle: {
    color: "#fff",
    fontSize: 16,
    fontWeight: "700",
    marginBottom: 4,
  },
  confirmUrl: {
    color: "#00d9ff",
    fontSize: 13,
    fontFamily: "Courier",
    marginBottom: 12,
  },
  idRow: {
    flexDirection: "row",
    marginBottom: 6,
  },
  idLabel: {
    color: "#888",
    fontSize: 12,
    width: 80,
  },
  idValue: {
    color: "#fff",
    fontSize: 12,
    flex: 1,
  },
  idValueMono: {
    fontFamily: "Courier",
  },
  confirmDanger: {
    color: "#ff6666",
    fontSize: 13,
    lineHeight: 18,
    marginTop: 10,
  },
  confirmWarn: {
    color: "#ffb347",
    fontSize: 13,
    lineHeight: 18,
    marginTop: 10,
  },
  confirmActions: {
    flexDirection: "row",
    marginTop: 16,
  },
  cancelButton: {
    flex: 1,
    paddingVertical: 12,
    alignItems: "center",
    borderRadius: 8,
    borderWidth: 1,
    borderColor: "#0f3460",
    marginRight: 8,
  },
  cancelButtonText: {
    color: "#888",
    fontSize: 15,
    fontWeight: "600",
  },
  trustButton: {
    flex: 1,
    backgroundColor: "#00ff88",
    paddingVertical: 12,
    alignItems: "center",
    borderRadius: 8,
    marginLeft: 8,
  },
  trustButtonDisabled: {
    opacity: 0.4,
  },
  trustButtonText: {
    color: "#1a1a2e",
    fontSize: 15,
    fontWeight: "700",
  },
});
