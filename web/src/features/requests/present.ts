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
import type { ChipTone } from "@/components/badge/chips";
import { isHumanName } from "@/people/names";
import {
  actorSegment,
  seg,
  type Segment,
  type Segments,
} from "@/people/segments";
import { isRequester } from "./actions";

export const STATE_LABELS: Record<RequestState, string> = {
  open: "Open",
  drafting: "Drafting",
  draft_ready: "Draft ready",
  changes_requested: "Changes requested",
  approved: "Approved",
  closed: "Closed",
};

export interface StateChip {
  label: string;
  tone: ChipTone;
}

/** The state as a chip: gold only for a draft that waits for the signed-in requester, green once approved, the neutral fill otherwise. */
export function stateChip(
  request: RequestSummary,
  human: string | null,
): StateChip {
  if (request.state === "draft_ready" && isRequester(request, human)) {
    return { label: "Draft ready · waits for you", tone: "warn" };
  }
  return {
    label: STATE_LABELS[request.state],
    tone: request.state === "approved" ? "ok" : "neutral",
  };
}

/** A request no orchestrator has sorted yet is `unsorted`. */
export type KindKey = RequestKind | "unsorted";

export const KIND_LABELS: Record<KindKey, string> = {
  bug: "Bug",
  feature: "Feature",
  question: "Question",
  unsorted: "Unsorted",
};

export function kindOf(request: RequestSummary): KindKey {
  return request.kind ?? "unsorted";
}

export interface Heading {
  text: string;
  /** The start of the message stands in for a title no orchestrator has written yet. */
  excerpt: boolean;
}

/** A list row's title: the title, or the start of the message while the request has none. */
export function headingOf(request: RequestSummary): Heading {
  return request.title === null
    ? { text: request.excerpt, excerpt: true }
    : { text: request.title, excerpt: false };
}

/** The request page's title: the title, or `Request RQ-4` while the request has none, so its message shows only once below. */
export function pageTitle(request: RequestSummary): string {
  return request.title ?? `Request ${request.id}`;
}

/** The line under a row's title: `RQ-4 · Bug`, then `· no title yet` or the age when given. */
export function rowSubLine(request: RequestSummary, age?: string): string {
  const parts = [request.id, KIND_LABELS[kindOf(request)]];
  if (age !== undefined) parts.push(age);
  else if (request.title === null) parts.push("no title yet");
  return parts.join(" · ");
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

/** The status block's heading: what the request waits for and who acts next. */
export function statusHeading(
  detail: RequestDetail,
  human: string | null,
  now: number,
): Segments {
  const r = detail.request;
  const latest = r.latest_version;
  const v = latest === null ? "" : `v${String(latest)}`;
  switch (r.state) {
    case "open":
      return ["No orchestrator has claimed it yet"];
    case "drafting":
      return r.drafter === null
        ? [
            `An orchestrator is drafting${v ? ` the version after ${v}` : " the cards"}`,
          ]
        : seg`${actorSegment(r.drafter)} is drafting${v ? ` the version after ${v}` : " the cards"}`;
    case "draft_ready":
      return isRequester(r, human)
        ? [`${v} waits for your decision`]
        : seg`${v} waits for ${actorSegment(r.requester)}`;
    case "changes_requested":
      return seg`${v} went back to ${drafterSegment(r.drafter)}`;
    case "approved": {
      const tasks = detail.tasks;
      const made =
        tasks.length === 0
          ? ""
          : ` · ${joinIds(tasks)} ${tasks.length === 1 ? "was" : "were"} created`;
      return seg`${actorSegment(r.requester)} approved ${v} at ${formatAsOfMinute(now, r.updated_at)}${made}`;
    }
    case "closed":
      return [`Closed ${formatSince(now, r.updated_at)}`];
  }
}

/** The drafter as a segment, or plain words while no orchestrator has claimed the request. */
export function drafterSegment(drafter: string | null): Segment {
  return drafter === null ? "an orchestrator" : actorSegment(drafter);
}

/** The line under the page title: who asked and when, and that the request has no title yet. */
export function metaLine(detail: RequestDetail, now: number): Segment[] {
  const r = detail.request;
  const asked = seg`Requested by ${actorSegment(r.requester)} ${formatSince(now, r.created_at)}`;
  return r.title === null ? [...asked, " · No title yet"] : asked;
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

export interface ListSummary {
  /** `7 requests`. */
  total: string;
  /** `1 needs a human` while a draft waits for a human requester, else null. */
  waiting: string | null;
}

/** `7 requests` and `1 needs a human`: a draft waits for a human requester. */
export function listSummary(requests: readonly RequestSummary[]): ListSummary {
  const n = requests.length;
  const waiting = requests.filter(
    (r) => r.state === "draft_ready" && isHumanName(r.requester),
  ).length;
  return {
    total: `${String(n)} ${n === 1 ? "request" : "requests"}`,
    waiting:
      waiting === 0
        ? null
        : `${String(waiting)} ${waiting === 1 ? "needs" : "need"} a human`,
  };
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
