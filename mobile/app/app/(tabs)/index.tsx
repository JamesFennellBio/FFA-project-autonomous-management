import { router } from "expo-router";
import { useState } from "react";
import { Pressable, RefreshControl, ScrollView, Switch, View } from "react-native";

import { ago, formatValue, isOnline } from "../../src/format";
import { useHub } from "../../src/hub/context";
import { errorText } from "../../src/hub/native";
import type { ChannelRow, NodeRow } from "../../src/hub/types";
import { NotPaired } from "../../src/NotPaired";
import { Body, Card, Empty, ErrorText, Title, styles, usePalette } from "../../src/ui";
import { usePoll } from "../../src/usePoll";

interface Snapshot {
  nodes: NodeRow[];
  channels: ChannelRow[];
  time: number;
}

export default function Live() {
  const { hub } = useHub();
  const c = usePalette();
  const snap = usePoll<Snapshot>(
    hub
      ? async () => {
          const [nodes, channels, info] = [await hub.nodes(), await hub.channels(), await hub.info()];
          return { nodes, channels, time: info.time };
        }
      : null,
    5000,
  );
  // Output switches move right away; the next poll shows the acked state.
  const [pending, setPending] = useState<Record<string, number>>({});
  const [cmdError, setCmdError] = useState<string | null>(null);

  if (!hub) return <NotPaired />;

  const set = async (ch: ChannelRow, on: boolean) => {
    const key = `${ch.node}:${ch.channel}`;
    setPending((p) => ({ ...p, [key]: on ? 1 : 0 }));
    setCmdError(null);
    try {
      const r = await hub.command(ch.node, ch.channel, on ? 1 : 0);
      if (!r.node_online) setCmdError("Queued: the node hasn't been heard from yet, it gets it when it next reports.");
      await snap.refresh();
    } catch (e) {
      setCmdError(errorText(e));
    } finally {
      setPending((p) => {
        const next = { ...p };
        delete next[key];
        return next;
      });
    }
  };

  const data = snap.data;
  return (
    <ScrollView
      style={{ backgroundColor: c.bg }}
      contentContainerStyle={styles.screen}
      refreshControl={<RefreshControl refreshing={snap.loading && !data} onRefresh={() => void snap.refresh()} />}
    >
      <ErrorText>{snap.error}</ErrorText>
      <ErrorText>{cmdError}</ErrorText>
      {data && data.nodes.length === 0 && <Empty>No nodes yet. Pair one from the Pairing tab or the desktop app.</Empty>}
      {data?.nodes.map((n) => {
        const online = isOnline(n.last_seen, data.time);
        const chans = data.channels.filter((ch) => ch.node === n.id);
        return (
          <Card key={n.id}>
            <View style={[styles.row, { justifyContent: "space-between" }]}>
              <Title>{n.name}</Title>
              <Body small muted>
                {online ? "● online" : `offline · ${ago(n.last_seen, data.time)}`}
              </Body>
            </View>
            {chans.length === 0 && <Body muted small>Waiting for this node to report its channels.</Body>}
            {chans.map((ch) => {
              const key = `${ch.node}:${ch.channel}`;
              const shown = pending[key] ?? ch.value;
              return (
                <Pressable
                  key={key}
                  onPress={() => router.push(`/channel/${ch.node}/${ch.channel}`)}
                  style={({ pressed }) => [
                    styles.row,
                    { justifyContent: "space-between", paddingVertical: 8, opacity: pressed ? 0.6 : 1 },
                  ]}
                >
                  <View style={{ flex: 1 }}>
                    <Body>{ch.name}</Body>
                    <Body small muted>
                      {ch.ts ? ago(ch.ts, data.time) : "no reading yet"}
                    </Body>
                  </View>
                  {ch.output ? (
                    <Switch
                      value={shown === 1}
                      disabled={key in pending}
                      onValueChange={(on) => void set(ch, on)}
                      trackColor={{ true: c.accent }}
                      accessibilityLabel={`${ch.name} ${shown === 1 ? "on" : "off"}`}
                    />
                  ) : (
                    <Body>{formatValue(ch.kind, ch.value)}</Body>
                  )}
                </Pressable>
              );
            })}
          </Card>
        );
      })}
    </ScrollView>
  );
}
