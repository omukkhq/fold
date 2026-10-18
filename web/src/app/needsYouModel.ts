import type { NeedsYouItem, NeedsYouKind } from "@/api/workers";
import { taskPath } from "@/features/board/card";
import { messagesHref } from "@/features/messages/search";
import { asksText } from "@/features/workers/permission";
import { projectPath, withQuery } from "@/routes/paths";

/** A line of the list: a person who asks, or an item chip, then the words and the screen it opens. */
export interface NeedsYouLine {
  key: string;
  kind: NeedsYouKind;
  /** The agent that asks; null for an item the line names by its chip. */
  person: string | null;
  chip: string | null;
  text: string;
  task: string | null;
  /** The second line's title: the proposal title of a review, the request title of a draft, the task title of a human task, the reason of a blocker, the conflicting files of a human change, the files that hold the project folder back. */
  title: string | null;
  since: number;
  hint: string;
  href: string;
}

export interface NeedsYouGroup {
  kind: NeedsYouKind;
  title: string;
  lines: NeedsYouLine[];
}

const GROUPS: readonly { kind: NeedsYouKind; title: string }[] = [
  { kind: "question", title: "Questions" },
  { kind: "permission", title: "Permissions" },
  { kind: "human_review", title: "Reviews" },
  { kind: "request_draft", title: "Request drafts" },
  { kind: "human_task", title: "Human tasks" },
  { kind: "blocker", title: "Blockers" },
  { kind: "human_change", title: "Your changes" },
  { kind: "head_moved", title: "Git branch" },
  { kind: "folder_held", title: "Project folder" },
];

/** The items for this human: an item that waits for another named human is theirs. Before a name, every item shows. */
export function itemsFor(
  items: readonly NeedsYouItem[],
  human: string | null,
): NeedsYouItem[] {
  return items.filter(
    (item) => human === null || item.human === null || item.human === human,
  );
}

/** The title as the second line shows it, trimmed; null for a blank or missing title. The line cuts a long one to its width. */
export function titleText(title: string | null): string | null {
  const text = title?.trim() ?? "";
  return text === "" ? null : text;
}

/** The files an item names, such as the conflicting files of a human change, for the second line, which holds one short path: the first file, and how many more there are; null when none is named. */
export function pathsText(paths: readonly string[]): string | null {
  const [first] = paths;
  if (first === undefined) {
    return null;
  }
  return paths.length === 1
    ? first
    : `${first} and ${String(paths.length - 1)} more`;
}

/** `asks about Chart library` with the question's header, else what kind of answer it wants. */
export function questionText(
  item: Pick<NeedsYouItem, "header" | "choices">,
): string {
  if (item.header !== null) return `asks about ${item.header}`;
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
        hint: "opens the task",
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
        hint: "opens the task",
        href: taskPath(slug, item.task ?? ""),
      };
    case "question":
      return {
        ...base,
        person: item.agent,
        chip: null,
        text: questionText(item),
        hint: "opens the thread",
        href: messagesHref(slug, {
          thread: item.thread ?? item.message ?? undefined,
        }),
      };
    case "permission":
      return {
        ...base,
        person: item.agent,
        chip: null,
        text: asksText(item.tool ?? "a tool"),
        hint: "opens the permission panel",
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
        hint: "opens the history of main",
        href: projectPath(slug, "repository"),
      };
    case "head_moved":
      return {
        ...base,
        person: null,
        chip: null,
        task: null,
        title: titleText(item.reason),
        text: "fold does not update the project folder until the Git branch is back",
        hint: "opens the history of main",
        href: projectPath(slug, "repository"),
      };
    case "folder_held":
      return {
        ...base,
        person: null,
        chip: null,
        task: null,
        title: pathsText(item.paths),
        text: "fold does not update the project folder until you record your changes with fld record or take them back",
        hint: "opens the history of main",
        href: projectPath(slug, "repository"),
      };
    case "human_review":
      return {
        ...base,
        person: null,
        chip: item.proposal,
        text: "needs your review",
        hint: "opens the proposal",
        href: projectPath(slug, "proposals", item.proposal ?? ""),
      };
    default:
      return {
        ...base,
        person: null,
        chip: item.request,
        text: "has a draft ready to approve",
        hint: "opens the request",
        href: projectPath(slug, "requests", item.request ?? ""),
      };
  }
}

/** The list in group order: questions, permissions, reviews, request drafts, human tasks, blockers, human changes, the Git branch, the project folder; a group without items is left out. */
export function needsYouGroups(
  slug: string,
  items: readonly NeedsYouItem[],
): NeedsYouGroup[] {
  const lines = items.map((item, index) => lineOf(slug, item, index));
  return GROUPS.map((group) => ({
    ...group,
    lines: lines.filter((line) => line.kind === group.kind),
  })).filter((group) => group.lines.length > 0);
}

export function waitingText(count: number): string {
  return `${String(count)} waiting`;
}
