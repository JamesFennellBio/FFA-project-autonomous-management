import { useState } from "react";

import { ago } from "../../src/format";
import { useHub } from "../../src/hub/context";
import { errorText } from "../../src/hub/native";
import type { PairRequestRow } from "../../src/hub/types";
import { NotPaired } from "../../src/NotPaired";
import { Body, Button, Card, Empty, ErrorText, Field, Mono, Screen, Title } from "../../src/ui";
import { usePoll } from "../../src/usePoll";

// Approving a node from the phone works just like the desktop app: the hub
// passes it to the PC, which picks the node id. So the PC has to be on.

export default function Pairing() {
  const { hub } = useHub();
  const list = usePoll(hub ? () => hub.pairRequests() : null, 4000);
  if (!hub) return <NotPaired />;
  return (
    <Screen>
      <Card>
        <Title>Pair a node</Title>
        <Body muted small>
          Hold the node's BOOT button for 3 seconds. It shows up here within a few seconds. Check that the fingerprint
          matches the node's serial monitor, then approve it. The PC must be running.
        </Body>
      </Card>
      <ErrorText>{list.error}</ErrorText>
      {list.data && list.data.length === 0 && <Empty>No nodes are asking to pair right now.</Empty>}
      {list.data?.map((r) => <Request key={r.public_key} r={r} />)}
    </Screen>
  );
}

function Request({ r }: { r: PairRequestRow }) {
  const { hub } = useHub();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const approve = async () => {
    if (!hub) return;
    setBusy(true);
    setErr(null);
    try {
      await hub.approveNode(r.public_key, name.trim() || undefined);
      setMsg("Sent to the PC. The node pairs the next time it asks (a few seconds).");
    } catch (e) {
      setErr(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card>
      <Title>Data node ({r.transport === "lora" ? "LoRa" : "Wi-Fi"})</Title>
      <Mono>{r.fingerprint}</Mono>
      <Body small muted>Seen {ago(r.seen_at)}. Only approve it if this matches what the node shows.</Body>
      <Field label="Name" placeholder="e.g. Greenhouse 1" value={name} onChangeText={setName} />
      <Button primary title="Approve" busy={busy} disabled={msg != null} onPress={() => void approve()} />
      {msg && <Body small>{msg}</Body>}
      <ErrorText>{err}</ErrorText>
    </Card>
  );
}
