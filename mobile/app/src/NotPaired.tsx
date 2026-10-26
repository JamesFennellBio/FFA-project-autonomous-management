import { router } from "expo-router";

import { useHub } from "./hub/context";
import { Body, Button, Card, Screen, Title } from "./ui";

/** Shown on every tab until the phone is paired. */
export function NotPaired() {
  const { ready } = useHub();
  if (!ready) return <Screen>{null}</Screen>;
  return (
    <Screen>
      <Card>
        <Title>Not connected to a hub</Title>
        <Body muted>Pair this phone with your farm's hub to see readings, switch outputs and build recipes.</Body>
        <Button primary title="Set up" onPress={() => router.navigate("/settings")} />
      </Card>
    </Screen>
  );
}
