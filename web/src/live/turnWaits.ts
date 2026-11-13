import type { components } from "../api/schema";

export type UsageLimitKind = components["schemas"]["UsageLimitKind"];
export type TurnWaitReason = components["schemas"]["TurnWaitReason"];

const WAIT_WORDS: Readonly<Record<TurnWaitReason, string>> = {
  network: "waiting for the network",
  usage_limit: "waits for the usage limit",
};

/** What a worker whose idle turn waits does, in the lower-case words of a chip or a row: `waiting for the network` or `waits for the usage limit`. */
export function waitWords(reason: TurnWaitReason): string {
  return WAIT_WORDS[reason];
}

const LIMIT_WORDS: Readonly<Record<UsageLimitKind, string>> = {
  session: "session limit",
  weekly: "weekly limit",
  opus: "Opus limit",
  sonnet: "Sonnet limit",
  usage: "usage limit",
};

const LIMIT_NAMES: ReadonlyMap<string, string> = new Map(
  Object.entries(LIMIT_WORDS),
);

/** The limit in the words a sentence uses, `weekly limit` or `Opus limit`; `usage limit` for an unknown kind, such as one an older or newer daemon sends. */
export function limitWords(kind: unknown): string {
  const words = typeof kind === "string" ? LIMIT_NAMES.get(kind) : undefined;
  return words ?? "usage limit";
}
