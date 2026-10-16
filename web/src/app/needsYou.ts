import type { NeedsYouItem, NeedsYouKind } from "@/api/workers";
import { messagesHref } from "@/features/messages/search";
import { asksText } from "@/features/workers/permission";

/** A line of the list: a person who asks, or an item chip, then the words and the screen it opens. */
export interface NeedsYouLine {
  key: string;
  kind: NeedsYouKind;
  /** The agent that asks; null for an item the line names by its chip. */
  person: string | null;
  chip: string | null;
  text: string;
  task: string | null;
  /** The second line's title: the proposal title of a review, the request title of a draft. */
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

/** `asks about Chart library` with the question's header, else what kind of answer it wants. */
export function questionText(
  item: Pick<NeedsYouItem, "header" | "choices">,
): string {
  if (item.header !== null) return `asks about ${item.header}`;
  return item.choices.length > 0 ? "asks you to choose" : "asks a question";
}

function lineOf(slug: string, item: NeedsYouItem, index: number): NeedsYouLine {
  const base = {
    key: `${item.kind}:${item.message ?? item.question ?? item.proposal ?? item.request ?? String(index)}`,
    kind: item.kind,
    task: item.task,
    title:
      item.kind === "human_review" || item.kind === "request_draft"
        ? titleText(item.title)
        : null,
    since: item.since,
  };
  switch (item.kind) {
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
        href: `/p/${slug}/agents?question=${item.question ?? ""}`,
      };
    case "human_review":
      return {
        ...base,
        person: null,
        chip: item.proposal,
        text: "needs your review",
        hint: "opens the proposal",
        href: `/p/${slug}/proposals/${item.proposal ?? ""}`,
      };
    default:
      return {
        ...base,
        person: null,
        chip: item.request,
        text: "has a draft ready to approve",
        hint: "opens the request",
        href: `/p/${slug}/requests/${item.request ?? ""}`,
      };
  }
}

/** The list in group order: questions, permissions, reviews, request drafts; a group without items is left out. */
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
