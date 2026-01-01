/**
 * Root Layout
 *
 * Sets up navigation and global providers for the Botho mobile wallet.
 */

import { useEffect } from "react";
import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useWalletStore } from "../src/store/walletStore";

const queryClient = new QueryClient();

export default function RootLayout() {
  const checkSession = useWalletStore((state) => state.checkSession);

  // Check session on app mount and periodically
  useEffect(() => {
    checkSession();

    const interval = setInterval(() => {
      checkSession();
    }, 60 * 1000); // Check every minute

    return () => clearInterval(interval);
  }, [checkSession]);

  return (
    <QueryClientProvider client={queryClient}>
      <StatusBar style="light" />
      <Stack
        screenOptions={{
          headerStyle: {
            backgroundColor: "#1a1a2e",
          },
          headerTintColor: "#fff",
          headerTitleStyle: {
            fontWeight: "600",
          },
          contentStyle: {
            backgroundColor: "#1a1a2e",
          },
        }}
      >
        <Stack.Screen
          name="index"
          options={{
            title: "Botho Wallet",
          }}
        />
        <Stack.Screen
          name="unlock"
          options={{
            title: "Unlock Wallet",
            presentation: "modal",
          }}
        />
        <Stack.Screen
          name="setup"
          options={{
            title: "Setup Wallet",
            presentation: "modal",
          }}
        />
        <Stack.Screen
          name="transactions"
          options={{
            title: "Transaction History",
          }}
        />
        <Stack.Screen
          name="receive"
          options={{
            title: "Receive",
            presentation: "modal",
          }}
        />
        <Stack.Screen
          name="send"
          options={{
            title: "Send",
            presentation: "modal",
          }}
        />
      </Stack>
    </QueryClientProvider>
  );
}
