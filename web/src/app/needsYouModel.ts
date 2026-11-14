import type { NeedsYouItem, NeedsYouKind } from "@/api/workers";
import { taskPath } from "@/features/board/card";
import { messagesHref } from "@/features/messages/search";
import { asksText } from "@/features/workers/permission";
import { agentsText, type AgentChange } from "./agentChanges";
import { codeData, codeParts } from "@/format/codeText";
import { projectPath, withQuery } from "@/routes/paths";
import { activityText } from "@/live/workerActivity";
import { limitWords } from "@/live/turnWaits";
import { formatAsOfMinute } from "@/format/time";

/** A line of the list: a person who asks, or an item chip, then the words and the screen it opens, if any. */
export interface NeedsYouLine {
  key: string;
  kind: NeedsYouKind;
  /** The agent that asks; null for an item the line names by its chip. */
  person: string | null;
  chip: string | null;
  /** The words, with each command between backticks and the item's data escaped, for `CodeText`. */
  text: string;
  task: string | null;
  /** The second line's title: the proposal title of a review, the request title of a draft, the task title of a human task, the reason of a blocker, the conflicting files of a human change or of an unstash, the files that hold the project folder back, the file, line and problem of main's checks file, the command that brings the Git branch back, lets a worker reach fold again or lets the daemon find claude, Claude Code's error of a worker that cannot reach the model. Marked and escaped as `text` is. */
  title: string | null;
  since: number;
  /** True when the title belongs in the sentence after a colon, as `P-4 needs your review: Audit log`; otherwise it is the second line. */
  inline: boolean;
  /** What the line opens; null for a line that opens nothing. */
  href: string | null;
  /** The worker's change of the project folder that the line settles in place, by putting it aside, keeping it or recording it; null for every other line. */
  settle: AgentChange | null;
  /** The files of a line about a worker's tool call, shown in the path face on the second line in place of `title`; null for every other line. */
  files: FilesSummary | null;
  /** How many items of changed shared files the line's Dismiss ends, which is every one of them; null for every other line. */
  dismiss: number | null;
  /** The worker that the line's Restart worker starts again in a new session: a stalled worker, or the holder of a stale claim, when the daemon names restarting as the person's action; null for every other line, and for a worker whose session fold does not run. */
  restart: string | null;
  /** What the human can do from the line, as the daemon lists it for the item, such as `pause` for a usage limit. */
  actions: readonly NeedsYouActionKind[];
  /** The other accounts of the host that the line's Switch to buttons make the project's, by name, in the daemon's order; empty for every other line. */
  switchTo: readonly string[];
}

export type NeedsYouActionKind = NeedsYouItem["actions"][number]["action"];

/** The accounts the item's `switch_account` actions name, in their order; an entry without a name is left out. */
export function switchTargets(
  actions: readonly NeedsYouItem["actions"][number][],
): string[] {
  return actions.flatMap((entry) =>
    entry.action === "switch_account" && entry.account ? [entry.account] : [],
  );
}

/** The files of an item as the second line names them: how many, the first path and the count of the rest. */
export interface FilesSummary {
  count: string;
  first: string;
  more: number;
}

export type NeedsYouGroupId = "ask" | "wait" | "project";

export interface NeedsYouGroup {
  id: NeedsYouGroupId;
  title: string;
  lines: NeedsYouLine[];
}

/** The order of the kinds inside their group. */
const KIND_ORDER: readonly NeedsYouKind[] = [
  "question",
  "permission",
  "human_review",
  "request_draft",
  "human_task",
  "blocker",
  "human_change",
  "head_moved",
  "folder_held",
  "unstash_conflicted",
  "checks_file_invalid_on_main",
  "worker_unreachable",
  "worker_model_unreachable",
  "claude_not_found",
  "notify_target_off",
  "agent_change",
  "shared_changed",
  "worker_stalled",
  "usage_limit",
  "stale_claim",
];

/** Agents that ask, things that wait for the human's decision, and the project's own state, which includes workers fold cannot run. */
export const GROUP_OF: Readonly<Record<NeedsYouKind, NeedsYouGroupId>> = {
  question: "ask",
  permission: "ask",
  human_review: "wait",
  request_draft: "wait",
  human_task: "wait",
  blocker: "wait",
  human_change: "project",
  head_moved: "project",
  folder_held: "project",
  unstash_conflicted: "project",
  checks_file_invalid_on_main: "project",
  worker_unreachable: "project",
  worker_model_unreachable: "project",
  claude_not_found: "project",
  notify_target_off: "project",
  agent_change: "project",
  shared_changed: "project",
  worker_stalled: "wait",
  usage_limit: "project",
  stale_claim: "wait",
};

/** The kinds whose title is the item's own name, which the sentence ends with. */
const INLINE_KINDS: ReadonlySet<NeedsYouKind> = new Set([
  "human_review",
  "request_draft",
  "human_task",
  "blocker",
]);

const GROUPS: readonly { id: NeedsYouGroupId; title: string }[] = [
  { id: "ask", title: "Agents ask you" },
  { id: "wait", title: "Waiting for you" },
  { id: "project", title: "Project" },
];

/** The items for this human: an item that waits for another named human is theirs, and a question to named humans is only theirs, as their Messages count is. Before a name, every item shows. */
export function itemsFor(
  items: readonly NeedsYouItem[],
  human: string | null,
): NeedsYouItem[] {
  return items.filter(
    (item) =>
      human === null ||
      ((item.human === null || item.human === human) &&
        (item.humans.length === 0 || item.humans.includes(human))),
  );
}

/** How many proposals wait for this human's review: the `human_review` items of Needs you that are theirs, which the Proposals tab counts. */
export function reviewsWaitingFor(
  items: readonly NeedsYouItem[],
  human: string | null,
): number {
  return itemsFor(items, human).filter((item) => item.kind === "human_review")
    .length;
}

/** The title as the second line shows it, trimmed and escaped as data; null for a blank or missing title. The line cuts a long one to its width. */
export function titleText(title: string | null): string | null {
  const text = markedText(title);
  return text === null ? null : codeData(text);
}

/** A daemon text that marks its commands between backticks, trimmed; null for a blank or missing text. */
function markedText(text: string | null): string | null {
  const trimmed = text?.trim() ?? "";
  return trimmed === "" ? null : trimmed;
}

/** The files an item names, such as the conflicting files of a human change, for the second line, which holds one short path: the first file, and how many more there are; null when none is named. */
export function pathsText(paths: readonly string[]): string | null {
  const [first] = paths;
  if (first === undefined) {
    return null;
  }
  return paths.length === 1
    ? codeData(first)
    : `${codeData(first)} and ${String(paths.length - 1)} more`;
}

/** The files of a worker's tool call with their count, as `27 files`, `README.md` and 26 more; null when none is named. */
export function filesSummary(paths: readonly string[]): FilesSummary | null {
  const [first] = paths;
  if (first === undefined) return null;
  return {
    count: paths.length === 1 ? "1 file" : `${String(paths.length)} files`,
    first,
    more: paths.length - 1,
  };
}

/** The agents of a worker's change as the subject of its line: the one agent as the line's person, else the agents named in the words. */
function changedBy(agents: readonly string[]): {
  person: string | null;
  subject: string;
} {
  const [only] = agents;
  return agents.length === 1 && only !== undefined
    ? { person: only, subject: "" }
    : { person: null, subject: `${codeData(agentsText(agents))} ` };
}

/** `asks 3 questions` for a set of several, else `asks about Chart library` with the question's header, else what kind of answer it wants. */
export function questionText(
  item: Pick<NeedsYouItem, "header" | "choices" | "questions">,
): string {
  if (item.questions > 1) return `asks ${String(item.questions)} questions`;
  if (item.header !== null) return `asks about ${codeData(item.header)}`;
  return item.choices.length > 0 ? "asks you to choose" : "asks a question";
}

/** `text` as a sentence of its own, with a capital first letter, except fold's name, which is always written in lower case; null stays null. */
function sentence(text: string | null): string | null {
  if (text === null || text.startsWith("fold ")) {
    return text;
  }
  return `${text.charAt(0).toUpperCase()}${text.slice(1)}`;
}

/** What fold saw of a stalled worker, in the words of its card's chip; the line names its task after it. */
function stalledText(item: NeedsYouItem, now: number): string {
  const agent = item.agent ?? "a worker";
  return codeData(
    item.activity === null
      ? `@${agent} stalled`
      : activityText(item.activity, agent, now),
  );
}

/** The second line of an item whose workers' turns wait: who waits, then what fold does, as `@web-lead waits; fold runs each waiting turn again once the model answers`. */
export function waitersText(
  agents: readonly string[],
  next: string | null,
): string {
  const who = codeData(agentsText(agents));
  const waits = `${who} ${agents.length === 1 ? "waits" : "wait"}`;
  return next === null ? waits : `${waits}; ${next}`;
}

/** `Claude Code reached its weekly limit, which resets Mon 12:00am`: the limit and its reset in Claude Code's words, or the reset instant in local time when the message gave one, or no reset at all. */
export function limitText(
  item: Pick<NeedsYouItem, "limit" | "reset" | "resets_at">,
  now: number,
): string {
  const reached = `Claude Code reached its ${limitWords(item.limit)}`;
  const reset = markedText(item.reset);
  if (reset !== null) {
    return `${reached}, which resets ${codeData(reset)}`;
  }
  if (item.resets_at !== null) {
    return `${reached}, which resets at ${formatAsOfMinute(now, item.resets_at)}`;
  }
  return reached;
}

function lineOf(
  slug: string,
  item: NeedsYouItem,
  index: number,
  now: number,
): NeedsYouLine {
  const base = {
    key: `${item.kind}:${item.message ?? item.question ?? item.proposal ?? item.request ?? item.task ?? item.commit ?? String(index)}`,
    kind: item.kind,
    task: item.task,
    title:
      item.kind === "human_review" || item.kind === "request_draft"
        ? titleText(item.title)
        : null,
    since: item.since,
    inline: INLINE_KINDS.has(item.kind),
    settle: null,
    files: null,
    dismiss: null,
    restart: null,
    actions: item.actions.map((entry) => entry.action),
    switchTo: switchTargets(item.actions),
  };
  switch (item.kind) {
    case "human_task":
      return {
        ...base,
        person: null,
        chip: item.task,
        task: null,
        title: titleText(item.title),
        text: "is ready for a person to do",
        href: taskPath(slug, item.task ?? ""),
      };
    case "blocker":
      return {
        ...base,
        person: null,
        chip: item.task,
        task: null,
        title: titleText(item.reason),
        text: "has a blocker",
        href: taskPath(slug, item.task ?? ""),
      };
    case "question":
      return {
        ...base,
        person: item.agent,
        chip: null,
        text: questionText(item),
        href: messagesHref(slug, {
          thread: item.thread ?? item.message ?? undefined,
        }),
      };
    case "permission":
      return {
        ...base,
        person: item.agent,
        chip: null,
        text: asksText(codeData(item.tool ?? "a tool")),
        href: withQuery(projectPath(slug, "agents"), {
          question: item.question ?? "",
        }),
      };
    case "human_change":
      return {
        ...base,
        person: null,
        chip: item.commit === null ? null : item.commit.slice(0, 7),
        task: null,
        title: pathsText(item.paths),
        text: "conflicts with main; the main agent explains it",
        href: projectPath(slug, "repository"),
      };
    case "head_moved":
      return {
        ...base,
        person: null,
        chip: null,
        task: null,
        title: markedText(item.reason),
        text: "fold does not update the project folder until the Git branch is back",
        href: projectPath(slug, "repository"),
      };
    case "folder_held":
      return {
        ...base,
        person: null,
        chip: null,
        task: null,
        title: pathsText(item.paths),
        text: "Changes were made in your project. Undo the changes or save them using `fld record`",
        href: projectPath(slug, "repository"),
      };
    case "unstash_conflicted":
      return {
        ...base,
        person: null,
        chip: null,
        task: null,
        title: pathsText(item.paths),
        text: "Unstash conflicts. Fix the files and run `fld record`",
        href: projectPath(slug, "repository"),
      };
    case "checks_file_invalid_on_main":
      return {
        ...base,
        person: null,
        chip: null,
        task: null,
        title: titleText(item.title),
        text: "Integrations wait until the checks file on main is fixed",
        href: null,
      };
    case "worker_unreachable":
      return {
        ...base,
        person: item.agent,
        chip: null,
        task: null,
        title: markedText(item.reason),
        text: "cannot reach fold and stopped",
        href: projectPath(slug, "agents"),
      };
    case "claude_not_found":
      return {
        ...base,
        person: item.agent,
        chip: null,
        task: null,
        title: markedText(item.reason),
        text: "cannot start: the daemon finds no claude",
        href: projectPath(slug, "agents"),
      };
    case "worker_model_unreachable": {
      const check = markedText(item.reason);
      if (item.agent === null) {
        return {
          ...base,
          person: null,
          chip: null,
          task: null,
          title: waitersText(item.agents, check),
          text: "fold cannot reach the model",
          href: projectPath(slug, "agents"),
        };
      }
      return {
        ...base,
        person: item.agent,
        chip: null,
        task: null,
        title: titleText(item.title),
        text:
          check === null
            ? "cannot reach the model"
            : `cannot reach the model: ${check}`,
        href: projectPath(slug, "agents"),
      };
    }
    case "notify_target_off":
      return {
        ...base,
        person: null,
        chip: item.target,
        task: null,
        title: titleText(item.title),
        text: "fold turned this notice target off after its sends failed; a test that gets through turns it on again",
        href: projectPath(slug, "settings"),
      };
    case "agent_change": {
      const by = changedBy(item.agents);
      const they = by.person === null ? "they" : "it";
      return {
        ...base,
        person: by.person,
        chip: null,
        task: null,
        files: filesSummary(item.paths),
        text: `${by.subject}changed files in the project folder while ${they} worked`,
        href: null,
        settle: { paths: item.paths, agents: item.agents },
      };
    }
    case "shared_changed":
      return {
        ...base,
        person: item.agent,
        chip: null,
        task: null,
        files: filesSummary(item.paths),
        text:
          item.paths.length === 1
            ? "changed a shared file while it worked. Check the file, then dismiss this"
            : "changed shared files while it worked. Check the files, then dismiss this",
        href: null,
      };
    case "worker_stalled":
      return {
        ...base,
        person: null,
        chip: null,
        title: sentence(markedText(item.reason)),
        text: stalledText(item, now),
        href: taskPath(slug, item.task ?? ""),
        restart:
          item.activity?.supervised === true ? (item.agent ?? null) : null,
      };
    case "usage_limit":
      return {
        ...base,
        person: null,
        chip: null,
        task: null,
        title: waitersText(item.agents, markedText(item.reason)),
        text: limitText(item, now),
        href: null,
      };
    case "stale_claim":
      return {
        ...base,
        person: item.agent,
        chip: null,
        // A review claim names its proposal, which already says the task.
        task: item.proposal === null ? item.task : null,
        title: sentence(markedText(item.reason)),
        text:
          item.proposal === null
            ? "holds a stale claim"
            : `holds a stale review claim on ${codeData(item.proposal)}`,
        href: taskPath(slug, item.task ?? ""),
        restart: item.action === "restart_worker" ? (item.agent ?? null) : null,
      };
    case "human_review":
      return {
        ...base,
        person: null,
        chip: item.proposal,
        text: "needs your review",
        href: projectPath(slug, "proposals", item.proposal ?? ""),
      };
    default:
      return {
        ...base,
        person: null,
        chip: item.request,
        text: "has a draft ready to approve",
        href: projectPath(slug, "requests", item.request ?? ""),
      };
  }
}

/** The list in three groups, Agents ask you, Waiting for you and Project, each in kind order and then in the order the daemon lists the items; a group without items is left out. */
export function needsYouGroups(
  slug: string,
  items: readonly NeedsYouItem[],
  now = 0,
): NeedsYouGroup[] {
  const shared = items.filter((item) => item.kind === "shared_changed").length;
  const lines = items
    .map((item, index) => lineOf(slug, item, index, now))
    .map((line) =>
      line.kind === "shared_changed" ? { ...line, dismiss: shared } : line,
    )
    .sort((a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind));
  return GROUPS.map((group) => ({
    ...group,
    lines: lines.filter((line) => GROUP_OF[line.kind] === group.id),
  })).filter((group) => group.lines.length > 0);
}

/** The title as plain words, for the tooltip of a title the line cuts; null for a line without one. */
export function fullTitle(line: Pick<NeedsYouLine, "title">): string | null {
  if (line.title === null) return null;
  return codeParts(line.title)
    .map((part) => (typeof part === "string" ? part : part.code))
    .join("");
}
