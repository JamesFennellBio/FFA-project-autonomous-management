import { useEffect, useState } from "react";
import { Alert } from "react-native";

import { useHub } from "../../src/hub/context";
import { errorText } from "../../src/hub/native";
import { Body, Button, Card, ErrorText, Field, Mono, Screen, Title } from "../../src/ui";
import { usePoll } from "../../src/usePoll";

export default function Settings() {
  const h = useHub();
  const [addr, setAddr] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const info = usePoll(h.hub ? () => h.hub!.info() : null, 30_000);

  useEffect(() => {
    setAddr(h.hubAddr ?? "");
    setKey(h.hubKey ?? "");
  }, [h.hubAddr, h.hubKey]);

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    setErr(null);
    try {
      await f();
    } catch (e) {
      setErr(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const pair = () =>
    run(async () => {
      if (!addr.trim()) throw new Error("Enter the hub's address.");
      await h.pair(addr, key);
    });

  const forget = () =>
    Alert.alert(
      "Forget this hub?",
      "This phone stops talking to the hub. The hub still trusts this phone until you remove it under Paired phones in the desktop app.",
      [
        { text: "Cancel", style: "cancel" },
        { text: "Forget", style: "destructive", onPress: () => void h.forget() },
      ],
    );

  return (
    <Screen>
      <Card>
        <Title>This phone</Title>
        <Body small muted>Fingerprint (the desktop app shows it when you pair):</Body>
        <Mono>{h.phoneFingerprint ?? "…"}</Mono>
      </Card>

      {h.pairing ? (
        <Card>
          <Title>Hub</Title>
          <Body>
            Paired as phone {h.pairing.phoneId} with {h.hubAddr}
          </Body>
          <Body small muted>Hub fingerprint:</Body>
          <Mono>{h.hubFingerprint}</Mono>
          {info.data && (
            <Body small muted>
              {info.data.nodes} node(s), {info.data.recipes}/{info.data.max_recipes} recipes
            </Body>
          )}
          <ErrorText>{info.error}</ErrorText>
          <Field
            label="Hub address (if it moved)"
            placeholder="192.168.1.20"
            value={addr}
            onChangeText={setAddr}
            keyboardType="numbers-and-punctuation"
          />
          <Button title="Save address" busy={busy} onPress={() => void run(() => h.setHub(addr, h.hubKey))} />
          <Button danger title="Forget this hub" onPress={forget} />
        </Card>
      ) : (
        <Card>
          <Title>Pair with a hub</Title>
          <Body small muted>
            The phone must be on the same Wi-Fi as the hub. Enter the hub's IP address (the port defaults to 7302).
          </Body>
          <Field
            label="Hub address"
            placeholder="192.168.1.20"
            value={addr}
            onChangeText={setAddr}
            keyboardType="numbers-and-punctuation"
          />
          <Field
            label={'Hub key ID (optional, the "phones, PC" line of `hub-node fingerprint`)'}
            placeholder="64 hex characters"
            value={key}
            onChangeText={setKey}
          />
          <Body small muted>
            With the key filled in, the phone refuses any other hub. Without it, it trusts the first hub that answers,
            so compare the hub fingerprint afterwards.
          </Body>
          <Button primary title={busy ? "Waiting for approval…" : "Pair with hub"} busy={busy} onPress={() => void pair()} />
          {busy && (
            <Body small>Now approve this phone on the desktop app's Pairing page (the fingerprint above).</Body>
          )}
        </Card>
      )}
      <ErrorText>{err}</ErrorText>
    </Screen>
  );
}
