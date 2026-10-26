import { Stack, useLocalSearchParams } from "expo-router";
import { useEffect, useRef, useState } from "react";
import { View, useWindowDimensions } from "react-native";
import Svg, { Line, Polyline } from "react-native-svg";

import { formatValue } from "../../../src/format";
import { useHub } from "../../../src/hub/context";
import type { ChannelRow } from "../../../src/hub/types";
import { NotPaired } from "../../../src/NotPaired";
import { Body, Button, Card, ErrorText, Screen, Title, styles, usePalette } from "../../../src/ui";
import { usePoll } from "../../../src/usePoll";

const RANGES = [
  { label: "1 h", secs: 3600 },
  { label: "24 h", secs: 86400 },
  { label: "7 d", secs: 7 * 86400 },
  { label: "30 d", secs: 30 * 86400 },
];

export default function History() {
  const params = useLocalSearchParams<{ node: string; channel: string }>();
  const node = Number(params.node);
  const channel = Number(params.channel);
  const { hub } = useHub();
  const c = usePalette();
  const { width } = useWindowDimensions();
  const [range, setRange] = useState(RANGES[1]);

  const data = usePoll(
    hub
      ? async () => {
          const info = await hub.info();
          const chans = await hub.channels();
          const ch = chans.find((x) => x.node === node && x.channel === channel) ?? null;
          const points = await hub.history(node, channel, info.time - range.secs);
          return { ch, points };
        }
      : null,
    30_000,
  );
  // Reload when the range changes (not on the first render: usePoll does that).
  const first = useRef(true);
  const { refresh } = data;
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    void refresh();
  }, [range, refresh]);

  if (!hub) return <NotPaired />;
  const ch: ChannelRow | null = data.data?.ch ?? null;
  const points = data.data?.points ?? [];
  const kind = ch?.kind ?? "unknown";

  const w = width - 64;
  const h = 180;
  let chart = null;
  if (points.length >= 2) {
    const ts = points.map((p) => p[0]);
    const vs = points.map((p) => p[1]);
    const [t0, t1] = [Math.min(...ts), Math.max(...ts)];
    let [v0, v1] = [Math.min(...vs), Math.max(...vs)];
    if (v0 === v1) [v0, v1] = [v0 - 1, v1 + 1];
    const xy = points
      .map(([t, v]) => `${((t - t0) / Math.max(1, t1 - t0)) * w},${h - ((v - v0) / (v1 - v0)) * (h - 8) - 4}`)
      .join(" ");
    chart = (
      <>
        <Svg width={w} height={h} accessibilityLabel={`${ch?.name ?? "Channel"} over the last ${range.label}`}>
          <Line x1={0} y1={h - 1} x2={w} y2={h - 1} stroke={c.border} strokeWidth={1} />
          <Polyline points={xy} fill="none" stroke={c.accent} strokeWidth={2} />
        </Svg>
        <View style={[styles.row, { justifyContent: "space-between" }]}>
          <Body small muted>min {formatValue(kind, Math.min(...vs))}</Body>
          <Body small muted>max {formatValue(kind, Math.max(...vs))}</Body>
          <Body small muted>last {formatValue(kind, vs[vs.length - 1])}</Body>
        </View>
      </>
    );
  }

  return (
    <Screen>
      <Stack.Screen options={{ title: ch?.name ?? "History" }} />
      <ErrorText>{data.error}</ErrorText>
      <Card>
        <Title>{ch ? `${ch.name}: ${formatValue(kind, ch.value)}` : "Loading…"}</Title>
        <View style={styles.row}>
          {RANGES.map((r) => (
            <View key={r.label} style={{ flex: 1 }}>
              <Button
                title={r.label}
                primary={r === range}
                onPress={() => setRange(r)}
              />
            </View>
          ))}
        </View>
        {chart ?? <Body muted>{data.loading ? "Loading…" : "Not enough readings in this range yet."}</Body>}
        <Body small muted>The hub keeps 30 days. Older history is on the PC.</Body>
      </Card>
    </Screen>
  );
}
