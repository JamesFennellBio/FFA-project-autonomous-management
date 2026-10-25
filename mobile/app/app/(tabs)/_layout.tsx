import Ionicons from "@expo/vector-icons/Ionicons";
import { Tabs } from "expo-router";
import type { ComponentProps } from "react";

import { usePalette } from "../../src/ui";

type Icon = ComponentProps<typeof Ionicons>["name"];

const TABS: { name: string; title: string; icon: Icon }[] = [
  { name: "index", title: "Live", icon: "pulse" },
  { name: "alerts", title: "Alerts", icon: "notifications-outline" },
  { name: "recipes", title: "Recipes", icon: "extension-puzzle-outline" },
  { name: "pairing", title: "Pairing", icon: "link-outline" },
  { name: "settings", title: "Settings", icon: "settings-outline" },
];

export default function TabsLayout() {
  const c = usePalette();
  return (
    <Tabs
      screenOptions={{
        headerStyle: { backgroundColor: c.card },
        headerTintColor: c.text,
        tabBarStyle: { backgroundColor: c.card, borderTopColor: c.border },
        tabBarActiveTintColor: c.accent,
        tabBarInactiveTintColor: c.muted,
        sceneStyle: { backgroundColor: c.bg },
      }}
    >
      {TABS.map((t) => (
        <Tabs.Screen
          key={t.name}
          name={t.name}
          options={{
            title: t.title,
            tabBarIcon: ({ color, size }) => <Ionicons name={t.icon} color={color} size={size} />,
          }}
        />
      ))}
    </Tabs>
  );
}
