import { Stack, router, useLocalSearchParams } from "expo-router";
import { useEffect, useMemo, useState } from "react";
import { Alert, Switch, View, useColorScheme } from "react-native";
import { WebView } from "react-native-webview";

import { unitFor } from "../../src/format";
import { useHub } from "../../src/hub/context";
import { errorText } from "../../src/hub/native";
import type { ChannelRow, NodeRow, Recipe } from "../../src/hub/types";
import { NotPaired } from "../../src/NotPaired";
import { editorHtml, editorInitScript, type EditorChannel, type EditorMessage } from "../../src/recipes/editorHtml";
import { Body, Button, ErrorText, Field, styles, usePalette } from "../../src/ui";

export default function RecipeEditor() {
  const { id: idParam } = useLocalSearchParams<{ id: string }>();
  const id = idParam === "new" ? 0 : Number(idParam);
  const { hub } = useHub();
  const c = usePalette();
  const dark = useColorScheme() === "dark";

  const [loaded, setLoaded] = useState<{ nodes: NodeRow[]; channels: ChannelRow[]; recipe: Recipe | null } | null>(null);
  const [name, setName] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [last, setLast] = useState<EditorMessage | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!hub) return;
    void (async () => {
      try {
        const nodes = await hub.nodes();
        const channels = await hub.channels();
        const recipe = id ? await hub.getRecipe(id) : null;
        if (recipe) {
          setName(recipe.name);
          setEnabled(recipe.enabled);
        }
        setLoaded({ nodes, channels, recipe });
      } catch (e) {
        setErr(errorText(e));
      }
    })();
  }, [hub, id]);

  // Built once, when the data arrives; the WebView keeps its own state after that.
  const page = useMemo(() => {
    if (!loaded) return null;
    const nodeName = (n: number) => loaded.nodes.find((x) => x.id === n)?.name ?? `Node ${n}`;
    const toEditor = (ch: ChannelRow): EditorChannel => {
      const u = unitFor(ch.kind);
      return {
        node: ch.node,
        channel: ch.channel,
        label: `${nodeName(ch.node)} · ${ch.name}`,
        kind: ch.kind,
        scale: u.scale,
        unit: ch.output || ch.kind === "digital_in" ? "0/1" : u.label,
      };
    };
    return {
      html: editorHtml(dark),
      init: editorInitScript({
        sensors: loaded.channels.map(toEditor),
        outputs: loaded.channels.filter((ch) => ch.output).map(toEditor),
        recipe: loaded.recipe,
      }),
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded]);

  if (!hub) return <NotPaired />;

  const save = async () => {
    if (!last || last.type !== "recipe") {
      setErr(last?.type === "error" ? last.message : "Build the recipe first.");
      return;
    }
    if (!name.trim()) {
      setErr("Give the recipe a name.");
      return;
    }
    setBusy(true);
    setErr(null);
    try {
      await hub.saveRecipe({
        id,
        name: name.trim(),
        enabled,
        when: last.when,
        then: last.then,
        otherwise: last.otherwise,
      });
      router.back();
    } catch (e) {
      setErr(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = () =>
    Alert.alert("Delete this recipe?", "If it's currently on, its 'otherwise' part runs first.", [
      { text: "Cancel", style: "cancel" },
      {
        text: "Delete",
        style: "destructive",
        onPress: () =>
          void (async () => {
            try {
              await hub.deleteRecipe(id);
              router.back();
            } catch (e) {
              setErr(errorText(e));
            }
          })(),
      },
    ]);

  return (
    <View style={{ flex: 1, backgroundColor: c.bg }}>
      <Stack.Screen options={{ title: id ? "Edit recipe" : "New recipe" }} />
      <View style={{ padding: 12, gap: 8 }}>
        <Field label="Name" placeholder="e.g. Greenhouse too hot" value={name} onChangeText={setName} maxLength={32} />
        <View style={[styles.row, { justifyContent: "space-between" }]}>
          <Body>Enabled</Body>
          <Switch value={enabled} onValueChange={setEnabled} trackColor={{ true: c.accent }} />
        </View>
        {last?.type === "error" && <Body small muted>{last.message}</Body>}
        <ErrorText>{err}</ErrorText>
      </View>
      <View style={{ flex: 1 }}>
        {page ? (
          <WebView
            originWhitelist={["*"]}
            source={{ html: page.html }}
            injectedJavaScriptBeforeContentLoaded={page.init}
            onMessage={(e) => setLast(JSON.parse(e.nativeEvent.data) as EditorMessage)}
            // The editor is a local page; it never needs to load anything from the web.
            onShouldStartLoadWithRequest={(req) => !/^https?:/i.test(req.url)}
            setSupportMultipleWindows={false}
            style={{ backgroundColor: c.bg }}
          />
        ) : (
          <View style={{ padding: 16 }}>
            <Body muted>Loading channels…</Body>
          </View>
        )}
      </View>
      <View style={[styles.row, { padding: 12 }]}>
        {id ? (
          <View style={{ flex: 1 }}>
            <Button danger title="Delete" onPress={remove} />
          </View>
        ) : null}
        <View style={{ flex: 2 }}>
          <Button primary title="Save to hub" busy={busy} onPress={() => void save()} />
        </View>
      </View>
    </View>
  );
}
