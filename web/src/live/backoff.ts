export const BASE_DELAY_MS = 1_000;
export const MAX_DELAY_MS = 30_000;

/** Doubles from one second to a 30 second ceiling; jitter keeps between half and all of that delay. `random` returns a value in [0, 1). */
export function nextDelay(attempt: number, random: () => number): number {
  const exponent = Math.min(Math.max(attempt, 0), 16);
  const ceiling = Math.min(MAX_DELAY_MS, BASE_DELAY_MS * 2 ** exponent);
  return Math.round(ceiling * (0.5 + 0.5 * random()));
}
