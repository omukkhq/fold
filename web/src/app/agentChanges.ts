import { ApiError, apiErrorCode } from "@/api/errors";
import { failureText } from "@/format/failure";

/** The choices of a Needs you item about a worker's change of the project folder. */
export type SettleAction = "record" | "restore" | "ignore";

/** One reason a record waits for a confirmation, with the files it names. */
export interface ConfirmGroup {
  key: "emptied" | "empty_added" | "agent_changed";
  title: string;
  paths: string[];
  /** How many files the reason counts beyond `paths`. */
  more: number;
}

/** What a record holds back until the human confirms it: the folder's tree that the confirmation names, and the files of each reason. */
export interface RecordConfirmation {
  tree: string;
  groups: ConfirmGroup[];
}

const GROUP_TITLES: readonly [ConfirmGroup["key"], string][] = [
  ["emptied", "Emptied"],
  ["empty_added", "New and empty"],
  ["agent_changed", "Changed while an agent worked"],
];

/** The files the dialog lists of each reason; the rest are counted. */
export const LISTED_PATHS = 20;

function strings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
}

/** The confirmation a `record_needs_confirmation` refusal asks for, or null for any other outcome. */
export function confirmationOf(error: unknown): RecordConfirmation | null {
  if (
    !(error instanceof ApiError) ||
    error.code !== "record_needs_confirmation"
  ) {
    return null;
  }
  const tree = error.details.tree;
  if (typeof tree !== "string") return null;
  const groups = GROUP_TITLES.flatMap(([key, title]) => {
    const all = strings(error.details[key]);
    const counted = error.details[`${key}_count`];
    const count = typeof counted === "number" ? counted : all.length;
    const paths = all.slice(0, LISTED_PATHS);
    return count === 0
      ? []
      : [{ key, title, paths, more: Math.max(0, count - paths.length) }];
  });
  return { tree, groups };
}

/** The agents of the item as words: `reviewer-35`, `web-lead and backend`, or `3 agents`. */
export function agentsText(agents: readonly string[]): string {
  if (agents.length === 0) return "an agent";
  if (agents.length === 1) return agents[0] ?? "an agent";
  if (agents.length === 2) return `${agents[0] ?? ""} and ${agents[1] ?? ""}`;
  return `${String(agents.length)} agents`;
}

/** What each choice's button says while its write is in flight. */
export const PENDING_LABELS: Readonly<Record<SettleAction, string>> = {
  record: "Recording…",
  restore: "Restoring…",
  ignore: "Ignoring…",
};

/** What each choice's button says. */
export const ACTION_LABELS: Readonly<Record<SettleAction, string>> = {
  record: "Record",
  restore: "Restore",
  ignore: "Ignore",
};

/** The sentence for a refused choice; null for the confirmation, which the dialog shows instead. */
export function settleFailure(error: unknown): string | null {
  switch (apiErrorCode(error)) {
    case "record_needs_confirmation":
      return null;
    case "agent_change_not_found":
      return "No change of an agent waits any more.";
    default:
      return failureText(error);
  }
}
