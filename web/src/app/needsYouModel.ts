import type { NeedsYouItem, NeedsYouKind } from "@/api/workers";
import { taskPath } from "@/features/board/card";
import { messagesHref } from "@/features/messages/search";
import { asksText } from "@/features/workers/permission";
import { codeData, codeParts } from "@/format/codeText";
import { projectPath, withQuery } from "@/routes/paths";

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

/** `asks about Chart library` with the question's header, else what kind of answer it wants. */
export function questionText(
  item: Pick<NeedsYouItem, "header" | "choices">,
): string {
  if (item.header !== null) return `asks about ${codeData(item.header)}`;
  return item.choices.length > 0 ? "asks you to choose" : "asks a question";
}

function lineOf(slug: string, item: NeedsYouItem, index: number): NeedsYouLine {
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
        href: null,
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
): NeedsYouGroup[] {
  const lines = items
    .map((item, index) => lineOf(slug, item, index))
    .sort((a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind));
  return GROUPS.map((group) => ({
    ...group,
    lines: lines.filter((line) => GROUP_OF[line.kind] === group.id),
  })).filter((group) => group.lines.length > 0);
}

export function waitingText(count: number): string {
  return `${String(count)} waiting`;
}

/** The title as plain words, for the tooltip of a title the line cuts; null for a line without one. */
export function fullTitle(line: Pick<NeedsYouLine, "title">): string | null {
  if (line.title === null) return null;
  return codeParts(line.title)
    .map((part) => (typeof part === "string" ? part : part.code))
    .join("");
}
