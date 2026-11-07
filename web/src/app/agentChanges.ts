import { ApiError, apiErrorCode } from "@/api/errors";
import { failureText } from "@/format/failure";

/** The choices of a Needs you item about a worker's change of the project folder. */
export type SettleAction = "record" | "restore" | "ignore";

/** A write of a choice: a record names the tree the person confirmed. */
export interface Choice {
  action: SettleAction;
  confirm?: string;
}

/** The files and agents of the Needs you item that the choices settle. */
export interface AgentChange {
  paths: readonly string[];
  agents: readonly string[];
}

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

/** Every write of the project folder from the web shares this mutation key, so a screen can hold its own folder writes while another runs. */
export const folderWriteKey = (slug: string) => [slug, "folder-write"] as const;

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

/** The agents of the item as words, each by its `@name` as every agent reads: `@reviewer-35`, `@web-lead and @backend`, or `3 agents`. */
export function agentsText(agents: readonly string[]): string {
  const [first, second] = agents;
  if (first === undefined) return "an agent";
  if (second === undefined) return `@${first}`;
  if (agents.length === 2) return `@${first} and @${second}`;
  return `${String(agents.length)} agents`;
}

/** The label of the Dismiss button of a changed shared file, which ends every such item. */
export function dismissLabel(count: number): string {
  return count > 1 ? `Dismiss all ${String(count)}` : "Dismiss";
}

function filesCount(count: number): string {
  return count === 1 ? "1 file" : `${String(count)} files`;
}

/**
 * The dialog of a choice. Review shows the files with their diffs and the three choices. Put aside and Keep as mine ask before they write and keep the files the person saw when they chose. A record asks once the daemon names what it holds back; `changed` says the folder changed since the list the person saw.
 */
export type Asking =
  | { action: "review" | "restore" | "ignore"; change: AgentChange }
  | { action: "record"; confirmation: RecordConfirmation; changed: boolean };

export type ChoiceEvent =
  | {
      type: "ask";
      action: "review" | "restore" | "ignore";
      change: AgentChange;
    }
  | { type: "failed"; error: unknown }
  | { type: "done" }
  | { type: "close" };

/** The dialog after `event`. A refusal keeps the open dialog, so its error shows where the person looks; a `record_needs_confirmation` opens the record dialog or gives it the new list. */
export function nextAsking(
  asking: Asking | null,
  event: ChoiceEvent,
): Asking | null {
  switch (event.type) {
    case "ask":
      return { action: event.action, change: event.change };
    case "done":
    case "close":
      return null;
    case "failed": {
      const confirmation = confirmationOf(event.error);
      if (confirmation === null) return asking;
      const changed =
        asking?.action === "record" &&
        asking.confirmation.tree !== confirmation.tree;
      return { action: "record", confirmation, changed };
    }
  }
}

/** What the dialog of a choice says and what its confirm button does. */
export interface DialogText {
  title: string;
  description: string;
  confirm: string;
  pending: string;
  danger: boolean;
  /** A line above the files, such as the note that the folder changed. */
  notice: string | null;
}

/** The words of the open dialog. */
export function dialogText(asking: Asking): DialogText {
  switch (asking.action) {
    case "review": {
      const files = filesCount(asking.change.paths.length);
      return {
        title: `${files} changed while ${agentsText(asking.change.agents)} worked`,
        description:
          "Open a file to see its change against the revision the project folder holds, then choose what happens to these files.",
        confirm: "",
        pending: "",
        danger: false,
        notice: null,
      };
    }
    case "restore": {
      const files = filesCount(asking.change.paths.length);
      return {
        title: `Put ${files} aside?`,
        description: `fold puts these ${files} in stash@{0} and puts main's version back; a file main does not have leaves the folder. Your other changes stay. fld unstash brings them back.`,
        confirm: "Put files aside",
        pending: "Putting files aside…",
        danger: false,
        notice: null,
      };
    }
    case "ignore": {
      const files = filesCount(asking.change.paths.length);
      return {
        title: `Keep ${files} as yours?`,
        description: `fold stops marking these ${files} as a change of ${agentsText(asking.change.agents)}. They stay in the project folder as your own unrecorded change, and a later record does not ask about them again.`,
        confirm: "Keep as mine",
        pending: "Keeping files…",
        danger: false,
        notice: null,
      };
    }
    case "record":
      return {
        title: "Record these files?",
        description:
          "Check them first: fold found files that are often a mistake. This records the whole project folder on main, also your other unrecorded changes.",
        confirm: "Record anyway",
        pending: "Recording…",
        danger: true,
        notice: asking.changed
          ? "The folder changed; check the files again."
          : null,
      };
  }
}

/** The label of the line's Put aside button, which names whose files it puts aside. */
export function putAsideLabel(agents: readonly string[]): string {
  return agents.length > 1
    ? "Put the agents' files aside"
    : "Put the agent's files aside";
}

/** What the line's Record button says while the first record, which has no dialog yet, is in flight. */
export const RECORD_PENDING = "Recording…";

function reasonOf(error: unknown): string | null {
  if (!(error instanceof ApiError)) return null;
  const { reason } = error.details;
  return typeof reason === "string" ? reason : null;
}

const REASONS: Readonly<Record<string, string>> = {
  git_commits_wait:
    "The project's Git branch has commits that fold did not make. Run fld status, then try again.",
  head_moved:
    "The Git branch or HEAD of the project folder moved outside fold. Run fld status, then try again.",
  stash_conflicted:
    "The conflict of an unstash waits in the project folder. Fix those files first.",
  stash_markers:
    "Files still hold the conflict markers of an unstash. Fix those files first.",
  main_changed:
    "Main changed these files since the revision of the project folder. Run fld status, then try again.",
  changed_during_stash:
    "A file changed while fold put the files aside. Try again.",
};

/** The sentence for a refused choice, by its code and `details.reason`; null for the confirmation, which the record dialog shows instead. */
export function settleFailure(error: unknown): string | null {
  switch (apiErrorCode(error)) {
    case "record_needs_confirmation":
      return null;
    case "agent_change_not_found":
      return "No change of an agent waits any more.";
    case "head_moved":
      return "Main moved during every try to record. Try again.";
    case "git_failed":
      return "A Git command failed. Run fld status to see why.";
    case "worker_peer_refused":
      return "fold refuses this from a process of a worker or a check.";
    case "client_header_required":
    case "host_not_allowed":
      return "The daemon refused this page. Open fold at its own address, then try again.";
    case "invalid_request": {
      const reason = reasonOf(error);
      return (
        (reason === null ? undefined : REASONS[reason]) ?? failureText(error)
      );
    }
    default:
      return failureText(error);
  }
}

/** Where the error of the last choice shows: in the open dialog, else under the line. */
export function failureAt(
  asking: Asking | null,
  error: unknown,
  place: "dialog" | "line",
): string | null {
  if (error === null || error === undefined) return null;
  if ((asking === null) !== (place === "line")) return null;
  return settleFailure(error);
}

/** The alert of the open dialog: the refusal of its write, or the note that the folder changed since the list the person saw, which a new `record_needs_confirmation` brings. */
export function dialogAlert(
  asking: Asking | null,
  error: unknown,
): string | null {
  const failed = failureAt(asking, error, "dialog");
  if (failed !== null || asking === null || !error) return failed;
  return dialogText(asking).notice;
}

/** The write the confirm button of the open dialog sends; null for the dialog that shows a worker's change, whose footer holds the three choices. */
export function confirmChoice(asking: Asking | null): Choice | null {
  if (asking === null || asking.action === "review") return null;
  return asking.action === "record"
    ? { action: "record", confirm: asking.confirmation.tree }
    : { action: asking.action };
}
