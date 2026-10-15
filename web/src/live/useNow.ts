import { useEffect, useState } from "react";

/**
 * The one display clock: the current time in epoch milliseconds, renewed every `everyMs` while `enabled`.
 * It only reads the clock so relative times and countdowns move on screen; it never fetches, so live data still comes only from the stream.
 */
export function useNow(enabled: boolean, everyMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!enabled) {
      return;
    }
    const read = () => {
      setNow(Date.now());
    };
    // The time may be old when the clock is turned on, so it is read again at once.
    const first = setTimeout(read, 0);
    const id = setInterval(read, everyMs);
    return () => {
      clearTimeout(first);
      clearInterval(id);
    };
  }, [enabled, everyMs]);
  return now;
}
