/**
 * Faucet Screen
 *
 * Requests testnet coins for the current wallet address via the bridge's
 * `request_faucet`, shows the result, then polls the balance until it rises.
 *
 * Note: the faucet RPC is served by the faucet node. If the wallet is pointed
 * at a non-faucet seed node, the request may fail — the screen surfaces the
 * node's message and offers a shortcut to the node picker.
 */

import { useEffect, useRef, useState } from "react";
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ActivityIndicator,
} from "react-native";
import { useRouter } from "expo-router";
import { useWalletStore } from "../src/store/walletStore";
import { faucetNode } from "../src/config/nodes";
import type { FaucetResult } from "../src/types/wallet";

/** How long to keep polling the balance after a faucet request (ms). */
const POLL_TIMEOUT_MS = 90_000;
/** Interval between balance polls (ms). */
const POLL_INTERVAL_MS = 5_000;

type Phase =
  | { state: "idle" }
  | { state: "requesting" }
  | { state: "polling"; result: FaucetResult }
  | { state: "credited"; result: FaucetResult }
  | { state: "failed"; message: string };

export default function FaucetScreen() {
  const router = useRouter();
  const { address, balance, nodeUrl, requestFaucet, refreshBalance } =
    useWalletStore();
  const [phase, setPhase] = useState<Phase>({ state: "idle" });
  const pollTimer = useRef<ReturnType<typeof setInterval> | null>(null);

  const onFaucetNode = faucetNode()?.url === nodeUrl;

  // Cleanup polling on unmount.
  useEffect(() => {
    return () => {
      if (pollTimer.current) clearInterval(pollTimer.current);
    };
  }, []);

  const startPolling = (result: FaucetResult, baseline: bigint) => {
    setPhase({ state: "polling", result });
    const deadline = Date.now() + POLL_TIMEOUT_MS;

    pollTimer.current = setInterval(async () => {
      await refreshBalance();
      const current = useWalletStore.getState().balance?.picocredits ?? 0n;

      if (current > baseline) {
        if (pollTimer.current) clearInterval(pollTimer.current);
        setPhase({ state: "credited", result });
      } else if (Date.now() > deadline) {
        if (pollTimer.current) clearInterval(pollTimer.current);
        // Stop polling but keep the (successful) request result visible.
        setPhase({ state: "polling", result });
      }
    }, POLL_INTERVAL_MS);
  };

  const handleRequest = async () => {
    setPhase({ state: "requesting" });
    const baseline = balance?.picocredits ?? 0n;
    try {
      const result = await requestFaucet();
      if (result.success) {
        startPolling(result, baseline);
      } else {
        setPhase({
          state: "failed",
          message: result.message || "Faucet declined the request",
        });
      }
    } catch (error) {
      setPhase({
        state: "failed",
        message:
          error instanceof Error ? error.message : "Faucet request failed",
      });
    }
  };

  return (
    <View style={styles.container}>
      <View style={styles.content}>
        <Text style={styles.icon}>🚰</Text>
        <Text style={styles.title}>Testnet Faucet</Text>
        <Text style={styles.subtitle}>
          Get free testnet BTH sent to your wallet to try sends and receives.
        </Text>

        {address && (
          <View style={styles.addressBox}>
            <Text style={styles.addressLabel}>Your address</Text>
            <Text style={styles.addressText} numberOfLines={2}>
              {address.display}
            </Text>
          </View>
        )}

        <View style={styles.balanceBox}>
          <Text style={styles.balanceLabel}>Current balance</Text>
          <Text style={styles.balanceText}>
            {balance?.formatted ?? "0.000000 BTH"}
          </Text>
        </View>

        {!onFaucetNode && (
          <TouchableOpacity
            style={styles.warningBox}
            onPress={() => router.push("/node-picker")}
          >
            <Text style={styles.warningText}>
              You are not connected to the faucet node. Tap to switch nodes.
            </Text>
          </TouchableOpacity>
        )}

        {/* Status */}
        {phase.state === "requesting" && (
          <View style={styles.statusBox}>
            <ActivityIndicator color="#00d9ff" />
            <Text style={styles.statusText}>Requesting coins…</Text>
          </View>
        )}

        {phase.state === "polling" && (
          <View style={styles.statusBox}>
            <ActivityIndicator color="#00d9ff" />
            <Text style={styles.statusText}>
              Faucet sent {phase.result.amountFormatted}. Waiting for it to
              confirm…
            </Text>
          </View>
        )}

        {phase.state === "credited" && (
          <View style={styles.successBox}>
            <Text style={styles.successText}>
              Received {phase.result.amountFormatted}!
            </Text>
            {phase.result.txHash ? (
              <Text style={styles.txHash} numberOfLines={1}>
                tx: {phase.result.txHash}
              </Text>
            ) : null}
          </View>
        )}

        {phase.state === "failed" && (
          <View style={styles.errorBox}>
            <Text style={styles.errorText}>{phase.message}</Text>
          </View>
        )}

        <TouchableOpacity
          style={[
            styles.button,
            (phase.state === "requesting" || phase.state === "polling") &&
              styles.buttonDisabled,
          ]}
          onPress={handleRequest}
          disabled={phase.state === "requesting" || phase.state === "polling"}
        >
          <Text style={styles.buttonText}>
            {phase.state === "credited" ? "Request More" : "Request Coins"}
          </Text>
        </TouchableOpacity>
      </View>
    </View>
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
  },
  icon: {
    fontSize: 56,
    textAlign: "center",
    marginTop: 12,
    marginBottom: 8,
  },
  title: {
    color: "#fff",
    fontSize: 24,
    fontWeight: "700",
    textAlign: "center",
    marginBottom: 8,
  },
  subtitle: {
    color: "#888",
    fontSize: 14,
    textAlign: "center",
    marginBottom: 24,
    lineHeight: 20,
  },
  addressBox: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
    marginBottom: 16,
    borderWidth: 1,
    borderColor: "#0f3460",
  },
  addressLabel: {
    color: "#888",
    fontSize: 12,
    marginBottom: 6,
  },
  addressText: {
    color: "#00d9ff",
    fontSize: 12,
    fontFamily: "Courier",
  },
  balanceBox: {
    backgroundColor: "#16213e",
    borderRadius: 12,
    padding: 16,
    marginBottom: 16,
    borderWidth: 1,
    borderColor: "#0f3460",
    alignItems: "center",
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
  warningBox: {
    backgroundColor: "#3a2a00",
    borderRadius: 8,
    padding: 12,
    marginBottom: 16,
    borderWidth: 1,
    borderColor: "#7a5a00",
  },
  warningText: {
    color: "#ffcc00",
    fontSize: 13,
  },
  statusBox: {
    flexDirection: "row",
    alignItems: "center",
    backgroundColor: "#16213e",
    borderRadius: 8,
    padding: 14,
    marginBottom: 16,
  },
  statusText: {
    color: "#ccc",
    fontSize: 14,
    marginLeft: 12,
    flex: 1,
  },
  successBox: {
    backgroundColor: "#0a2a1a",
    borderRadius: 8,
    padding: 14,
    marginBottom: 16,
    borderWidth: 1,
    borderColor: "#00ff88",
  },
  successText: {
    color: "#00ff88",
    fontSize: 16,
    fontWeight: "600",
  },
  txHash: {
    color: "#7fffcf",
    fontSize: 11,
    fontFamily: "Courier",
    marginTop: 6,
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
    marginTop: "auto",
  },
  buttonDisabled: {
    opacity: 0.5,
  },
  buttonText: {
    color: "#1a1a2e",
    fontSize: 18,
    fontWeight: "700",
  },
});
