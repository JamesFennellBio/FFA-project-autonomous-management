import { router } from "expo-router";
import { useState } from "react";
import { Pressable, Switch, View } from "react-native";

import { useHub } from "../../src/hub/context";
import { errorText } from "../../src/hub/native";
import { NotPaired } from "../../src/NotPaired";
import { Body, Button, Card, Empty, ErrorText, Screen, Title, styles, usePalette } from "../../src/ui";
import { usePoll } from "../../src/usePoll";

export default function Recipes() {
  const { hub } = useHub();
  const c = usePalette();
  const list = usePoll(hub ? () => hub.recipes() : null, 15_000);
  const [err, setErr] = useState<string | null>(null);
  if (!hub) return <NotPaired />;

  const toggle = async (id: number, enabled: boolean) => {
    setErr(null);
    try {
      await hub.setRecipeEnabled(id, enabled);
      await list.refresh();
    } catch (e) {
      setErr(errorText(e));
    }
  };

  return (
    <Screen>
      <Card>
        <Title>If this, then that</Title>
        <Body muted small>
          Recipes run on the hub itself, so they keep working when the PC and this phone are off. Each one acts once when
          its condition becomes true, and its "otherwise" part runs once when it stops being true.
        </Body>
        <Button primary title="New recipe" onPress={() => router.push("/recipe/new")} />
      </Card>
      <ErrorText>{list.error ?? err}</ErrorText>
      {list.data && list.data.length === 0 && <Empty>No recipes yet.</Empty>}
      {list.data?.map((r) => (
        <Card key={r.id}>
          <View style={[styles.row, { justifyContent: "space-between" }]}>
            <Pressable style={{ flex: 1 }} onPress={() => router.push(`/recipe/${r.id}`)}>
              <Title>{r.name}</Title>
              <Body small muted>{r.enabled ? "On" : "Off"} · tap to edit</Body>
            </Pressable>
            <Switch
              value={r.enabled}
              onValueChange={(on) => void toggle(r.id, on)}
              trackColor={{ true: c.accent }}
              accessibilityLabel={`${r.name} ${r.enabled ? "on" : "off"}`}
            />
          </View>
        </Card>
      ))}
    </Screen>
  );
}
