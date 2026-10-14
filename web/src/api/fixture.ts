export const FIXTURE_DELAY_MS = 400;

/**
 * A development build answers from web/fixtures after a delay longer than the router's pending threshold, so the pending states are real.
 * A production build answers with `empty` and holds no fixture data. Pass the loader as `import.meta.env.DEV ? () => import("…") : undefined`
 * written inline: Vite drops the fixture module only where the condition is the literal flag.
 */
export async function fromFixture<M, T>(
  load: (() => Promise<M>) | undefined,
  pick: (module: M) => T,
  empty: T,
  delayMs: number = FIXTURE_DELAY_MS,
): Promise<T> {
  if (!load) {
    return empty;
  }
  const [module] = await Promise.all([
    load(),
    new Promise<void>((resolve) => {
      setTimeout(resolve, delayMs);
    }),
  ]);
  return structuredClone(pick(module));
}
