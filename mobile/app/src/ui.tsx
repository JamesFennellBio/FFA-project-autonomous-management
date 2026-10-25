import type { ReactNode } from "react";
import {
  ActivityIndicator,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
  useColorScheme,
  type TextInputProps,
} from "react-native";

const light = {
  bg: "#f4f6f3",
  card: "#ffffff",
  text: "#1b241c",
  muted: "#5f6b60",
  border: "#dde3dc",
  accent: "#2f7d32",
  accentText: "#ffffff",
  danger: "#b3261e",
  ok: "#2f7d32",
};
const dark: typeof light = {
  bg: "#111611",
  card: "#1b221b",
  text: "#e6ece5",
  muted: "#9aa69a",
  border: "#2c362c",
  accent: "#66bb6a",
  accentText: "#0b120b",
  danger: "#f2b8b5",
  ok: "#81c784",
};
export type Palette = typeof light;

export function usePalette(): Palette {
  return useColorScheme() === "dark" ? dark : light;
}

export function Screen({ children, scroll = true }: { children: ReactNode; scroll?: boolean }) {
  const c = usePalette();
  if (!scroll) return <View style={[styles.screen, { backgroundColor: c.bg }]}>{children}</View>;
  return (
    <ScrollView style={{ backgroundColor: c.bg }} contentContainerStyle={styles.screen}>
      {children}
    </ScrollView>
  );
}

export function Card({ children }: { children: ReactNode }) {
  const c = usePalette();
  return <View style={[styles.card, { backgroundColor: c.card, borderColor: c.border }]}>{children}</View>;
}

export function Title({ children }: { children: ReactNode }) {
  const c = usePalette();
  return <Text style={[styles.title, { color: c.text }]}>{children}</Text>;
}

export function Body({ children, muted, small }: { children: ReactNode; muted?: boolean; small?: boolean }) {
  const c = usePalette();
  return <Text style={{ color: muted ? c.muted : c.text, fontSize: small ? 13 : 15, lineHeight: small ? 18 : 21 }}>{children}</Text>;
}

/** Fingerprints are compared by eye, so they get a monospace font. */
export function Mono({ children }: { children: ReactNode }) {
  const c = usePalette();
  return <Text style={[styles.mono, { color: c.text }]}>{children}</Text>;
}

export function ErrorText({ children }: { children: ReactNode }) {
  const c = usePalette();
  if (!children) return null;
  return <Text style={{ color: c.danger, fontSize: 14 }}>{children}</Text>;
}

export function Button({
  title,
  onPress,
  primary,
  danger,
  disabled,
  busy,
}: {
  title: string;
  onPress: () => void;
  primary?: boolean;
  danger?: boolean;
  disabled?: boolean;
  busy?: boolean;
}) {
  const c = usePalette();
  const bg = primary ? c.accent : "transparent";
  const fg = primary ? c.accentText : danger ? c.danger : c.accent;
  return (
    <Pressable
      accessibilityRole="button"
      onPress={onPress}
      disabled={disabled || busy}
      style={({ pressed }) => [
        styles.button,
        { backgroundColor: bg, borderColor: primary ? bg : c.border, opacity: disabled ? 0.5 : pressed ? 0.7 : 1 },
      ]}
    >
      {busy ? <ActivityIndicator color={fg} /> : <Text style={{ color: fg, fontWeight: "600", fontSize: 15 }}>{title}</Text>}
    </Pressable>
  );
}

export function Field(props: TextInputProps & { label: string }) {
  const c = usePalette();
  const { label, style, ...rest } = props;
  return (
    <View style={{ gap: 4 }}>
      <Text style={{ color: c.muted, fontSize: 13 }}>{label}</Text>
      <TextInput
        placeholderTextColor={c.muted}
        autoCapitalize="none"
        autoCorrect={false}
        style={[styles.input, { color: c.text, borderColor: c.border, backgroundColor: c.card }, style]}
        {...rest}
      />
    </View>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return (
    <View style={{ paddingVertical: 32, alignItems: "center" }}>
      <Body muted>{children}</Body>
    </View>
  );
}

export const styles = StyleSheet.create({
  screen: { padding: 16, gap: 12, flexGrow: 1 },
  card: { borderWidth: StyleSheet.hairlineWidth, borderRadius: 12, padding: 14, gap: 8 },
  title: { fontSize: 17, fontWeight: "600" },
  mono: { fontFamily: "monospace", fontSize: 15, letterSpacing: 0.5 },
  button: {
    borderWidth: 1,
    borderRadius: 10,
    paddingVertical: 11,
    paddingHorizontal: 16,
    alignItems: "center",
    justifyContent: "center",
    minHeight: 44,
  },
  input: { borderWidth: 1, borderRadius: 10, paddingHorizontal: 12, paddingVertical: 10, fontSize: 16 },
  row: { flexDirection: "row", alignItems: "center", gap: 12 },
});
