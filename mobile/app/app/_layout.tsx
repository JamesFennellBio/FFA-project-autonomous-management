import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";

import { HubProvider } from "../src/hub/context";
import { usePalette } from "../src/ui";

export default function RootLayout() {
  const c = usePalette();
  return (
    <HubProvider>
      <StatusBar style="auto" />
      <Stack
        screenOptions={{
          headerStyle: { backgroundColor: c.card },
          headerTintColor: c.text,
          contentStyle: { backgroundColor: c.bg },
        }}
      >
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        <Stack.Screen name="channel/[node]/[channel]" options={{ title: "History" }} />
        <Stack.Screen name="recipe/[id]" options={{ title: "Recipe" }} />
      </Stack>
    </HubProvider>
  );
}
