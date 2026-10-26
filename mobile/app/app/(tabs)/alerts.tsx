import { useState } from "react";

import { ago, describeEvent } from "../../src/format";
import { useHub } from "../../src/hub/context";
import { errorText } from "../../src/hub/native";
import type { ChannelRow, HubEvent } from "../../src/hub/types";
import { NotPaired } from "../../src/NotPaired";
import { Body, Button, Card, Empty, ErrorText, Screen } from "../../src/ui";
import { usePoll } from "../../src/usePoll";

export default function Alerts() {
  const { hub } = useHub();
  const first = usePoll(
    hub
      ? async () => {
          const [page, channels] = [await hub.events(), await hub.channels()];
          return { page, channels };
        }
      : null,
    15_000,
  );
  // Older pages, loaded on request.
  const [older, setOlder] = useState<{ events: HubEvent[]; next: number | null } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (!hub) return <NotPaired />;

  const channels: ChannelRow[] = first.data?.channels ?? [];
  const name = (node: number, ch: number) =>
    channels.find((c) => c.node === node && c.channel === ch)?.name ?? `node ${node} ch ${ch}`;
  const events = [...(first.data?.page.events ?? []), ...(older?.events ?? [])];
  const next = older ? older.next : (first.data?.page.next ?? null);

  const more = async () => {
    if (next == null) return;
    setBusy(true);
    setErr(null);
    try {
      const page = await hub.events(next);
      setOlder((o) => ({ events: [...(o?.events ?? []), ...page.events], next: page.next }));
    } catch (e) {
      setErr(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Screen>
      <ErrorText>{first.error ?? err}</ErrorText>
      {first.data && events.length === 0 && <Empty>No alerts in the last 30 days.</Empty>}
      {events.map((e) => (
        <Card key={e.id}>
          <Body>{describeEvent(e.event, name)}</Body>
          <Body small muted>{ago(e.event.ts)}</Body>
        </Card>
      ))}
      {next != null && <Button title="Older" busy={busy} onPress={() => void more()} />}
      <Body small muted>Alerts older than 30 days are in the desktop app.</Body>
    </Screen>
  );
}
