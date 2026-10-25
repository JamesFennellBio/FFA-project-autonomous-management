import { useFocusEffect } from "expo-router";
import { useCallback, useRef, useState } from "react";

import { errorText } from "./hub/native";

/**
 * Runs `fn` now and every `ms` while the screen is focused. Every poll is a
 * few encrypted round trips (and uses up session messages), so screens that
 * aren't visible don't poll.
 */
export function usePoll<T>(fn: (() => Promise<T>) | null, ms: number) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const fnRef = useRef(fn);
  fnRef.current = fn;

  const refresh = useCallback(async () => {
    const f = fnRef.current;
    if (!f) return;
    setLoading(true);
    try {
      setData(await f());
      setError(null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLoading(false);
    }
  }, []);

  const enabled = fn != null;
  useFocusEffect(
    useCallback(() => {
      if (!enabled) return undefined;
      void refresh();
      const t = setInterval(() => void refresh(), ms);
      return () => clearInterval(t);
    }, [enabled, ms, refresh]),
  );

  return { data, error, loading, refresh };
}
