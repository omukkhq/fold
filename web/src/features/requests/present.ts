import type {
  DraftCard,
  DraftEdge,
  RequestComment,
  RequestDetail,
  RequestDraft,
  RequestKind,
  RequestState,
  RequestSummary,
} from "@/api/requests";
import type { TaskStatus } from "@/api/board";
import { formatAsOfMinute, formatSince } from "@/format/time";
import { displayName } from "@/features/proposals/present";
import { isHumanName } from "@/people/names";

export { displayName };

export interface StateSpec {
  label: string;
  /** The pill's fill and text. */
  pill: string;
  /** The dot's fill. */
  dot: string;
}

export const STATE_SPECS: Record<RequestState, StateSpec> = {
  open: {
    label: "Open",
    pill: "bg-sunken text-text-muted",
    dot: "bg-status-todo",
  },
  drafting: {
    label: "Drafting",
    pill: "bg-primary-soft text-primary-text",
    dot: "bg-status-progress",
  },
  draft_ready: {
    label: "Draft ready",
    pill: "bg-secondary-soft text-secondary-text",
    dot: "bg-status-review",
  },
  changes_requested: {
    label: "Changes requested",
    pill: "bg-warning-soft text-warning",
    dot: "bg-warning",
  },
  approved: {
    label: "Approved",
    pill: "bg-success-soft text-text",
    dot: "bg-success",
  },
  closed: {
    label: "Closed",
    pill: "bg-sunken text-text-muted",
    dot: "bg-status-backlog",
  },
};

/** A request no orchestrator has sorted yet is `unsorted`. */
export type KindKey = RequestKind | "unsorted";

export interface KindSpec {
  label: string;
  classes: string;
}

export const KIND_SPECS: Record<KindKey, KindSpec> = {
  bug: { label: "Bug", classes: "bg-danger-soft text-danger" },
  feature: { label: "Feature", classes: "bg-primary-soft text-primary-text" },
  question: {
    label: "Question",
    classes: "bg-secondary-soft text-secondary-text",
  },
  unsorted: { label: "Unsorted", classes: "bg-sunken text-text-muted" },
};

export function kindOf(request: RequestSummary): KindKey {
  return request.kind ?? "unsorted";
}

export interface Heading {
  text: string;
  /** The start of the message stands in for a title no orchestrator has written yet. */
  muted: boolean;
}

export function headingOf(request: RequestSummary): Heading {
  return request.title === null
    ? { text: request.excerpt, muted: true }
    : { text: request.title, muted: false };
}

/** `18 KiB`, `14.2 MiB`: the sizes the dialog checks are binary units. */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${String(bytes)} B`;
  if (bytes < 1024 * 1024) return `${String(Math.round(bytes / 1024))} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

/** `T-12`, `T-12 and T-13`, `T-12, T-13 and T-14`. */
export function joinIds(ids: readonly string[]): string {
  if (ids.length <= 1) return ids.join("");
  return `${ids.slice(0, -1).join(", ")} and ${ids.at(-1) ?? ""}`;
}

function stateClause(detail: RequestDetail, now: number): string {
  const r = detail.request;
  const latest = r.latest_version;
  const v = latest === null ? "" : `v${String(latest)}`;
  switch (r.state) {
    case "open":
      return "no orchestrator has claimed it yet";
    case "drafting":
      return `${displayName(r.drafter ?? "an orchestrator")} is drafting${v ? ` the version after ${v}` : " the cards"}`;
    case "draft_ready":
      return `${v} waits for the requester`;
    case "changes_requested":
      return `${v} went back to the drafter`;
    case "approved": {
      const tasks = detail.tasks;
      const made =
        tasks.length === 0
          ? ""
          : ` · ${joinIds(tasks)} ${tasks.length === 1 ? "was" : "were"} created`;
      return `${displayName(r.requester)} approved ${v} at ${formatAsOfMinute(now, r.updated_at)}${made}`;
    }
    case "closed":
      return `closed ${formatSince(now, r.updated_at)}`;
  }
}

/** The line under the heading: who asked and when, who drafts, and who acts next. */
export function metaLine(detail: RequestDetail, now: number): string {
  const r = detail.request;
  const parts = [
    `Requested by ${displayName(r.requester)} ${formatSince(now, r.created_at)}`,
  ];
  if (
    r.drafter !== null &&
    (r.state === "draft_ready" || r.state === "changes_requested")
  ) {
    parts.push(`${displayName(r.drafter)} is the drafter`);
  }
  parts.push(stateClause(detail, now));
  return parts.join(" · ");
}

export function versionText(
  shown: number,
  latest: number,
  state: RequestState,
): string {
  const approved =
    state === "approved" && shown === latest ? " · approved" : "";
  return `Version ${String(shown)} of ${String(latest)}${approved}`;
}

/** `wrote v2 at 14:02 · Note: ...`. */
export function authorLine(draft: RequestDraft, now: number): string {
  const wrote = `wrote v${String(draft.version)} at ${formatAsOfMinute(now, draft.created_at)}`;
  const note = draft.note?.trim();
  return note ? `${wrote} · Note: ${note}` : wrote;
}

export interface EdgeLine {
  dependent: string;
  prerequisite: string;
  /** `CSV download needs Report query endpoint`. */
  text: string;
}

export function edgeLines(
  edges: readonly DraftEdge[],
  cards: readonly DraftCard[],
): EdgeLine[] {
  const title = (key: string) => cards.find((c) => c.key === key)?.title ?? key;
  return edges.map((e) => ({
    dependent: e.dependent,
    prerequisite: e.prerequisite,
    text: `${title(e.dependent)} needs ${title(e.prerequisite)}`,
  }));
}

export function edgesSummary(count: number): string {
  if (count === 0) return "No edges · the cards can start in any order";
  const n = `${String(count)} ${count === 1 ? "edge" : "edges"}`;
  return `${n} · a card waits until the cards it needs are done`;
}

/** `1 on v1, none on v2`: the comments per draft version. */
export function commentsSummary(
  comments: readonly RequestComment[],
  drafts: readonly RequestDraft[],
): string {
  if (drafts.length === 0) return "none yet";
  return drafts
    .map((d) => {
      const n = comments.filter((c) => c.version === d.version).length;
      return `${n === 0 ? "none" : String(n)} on v${String(d.version)}`;
    })
    .join(", ");
}

export function threadSummary(count: number, id: string): string {
  if (count === 0) return `No messages yet about ${id}`;
  return `${String(count)} ${count === 1 ? "message" : "messages"} in the thread of ${id}`;
}

/** `7 requests · 1 needs a human`: a draft waits for a human requester. */
export function listSummary(requests: readonly RequestSummary[]): string {
  const n = requests.length;
  const total = `${String(n)} ${n === 1 ? "request" : "requests"}`;
  const waiting = requests.filter(
    (r) => r.state === "draft_ready" && isHumanName(r.requester),
  ).length;
  return waiting === 0
    ? total
    : `${total} · ${String(waiting)} ${waiting === 1 ? "needs" : "need"} a human`;
}

/** The draft the screen shows: the chosen version, or the latest when none is chosen or the choice is gone. */
export function shownDraft(
  drafts: readonly RequestDraft[],
  chosen: number | null,
): RequestDraft | null {
  return drafts.find((d) => d.version === chosen) ?? drafts.at(-1) ?? null;
}

/** `Claimed by backend · draft v2 submitted 14:02`, from what the payload holds. */
export function drafterLine(
  drafts: readonly RequestDraft[],
  drafter: string,
  now: number,
): string {
  const own = drafts.filter((d) => d.author === drafter).at(-1);
  return own
    ? `Draft v${String(own.version)} submitted ${formatAsOfMinute(now, own.created_at)}`
    : "No draft submitted yet";
}

export interface CreatedTask {
  task: string;
  /** The draft card the task came from; tasks follow the card order of the approved version. */
  key: string | null;
  title: string;
  status: TaskStatus | null;
}

/** The tasks an approval created, each with its card and, when the board holds it, its title and calculated column. */
export function createdTasks(
  detail: RequestDetail,
  board:
    | readonly { task: string; title: string; status: TaskStatus }[]
    | undefined,
): CreatedTask[] {
  const approved = detail.drafts.at(-1);
  return detail.tasks.map((task, index) => {
    const card = approved?.cards[index];
    const onBoard = board?.find((c) => c.task === task);
    return {
      task,
      key: card?.key ?? null,
      title: onBoard?.title ?? card?.title ?? "",
      status: onBoard?.status ?? null,
    };
  });
}
