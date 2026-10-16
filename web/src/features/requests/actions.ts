import type { RequestState, RequestSummary } from "@/api/requests";
import { atName } from "@/people/names";
import { actorSegment, seg, type Segments } from "@/people/segments";

export type RequestAction = "approve" | "request_changes" | "edit" | "close";

/** The actions each state takes, as the daemon's state rule allows them: approving needs a draft that waits for the requester, asking for changes and editing need a draft, closing needs an open request. */
const BY_STATE: Record<RequestState, readonly RequestAction[]> = {
  open: ["close"],
  drafting: ["close"],
  draft_ready: ["approve", "request_changes", "edit", "close"],
  changes_requested: ["request_changes", "edit", "close"],
  approved: [],
  closed: [],
};

export function isFinal(state: RequestState): boolean {
  return state === "approved" || state === "closed";
}

/** The detail payload names the requester; the signed-in human is the requester when the names match. */
export function isRequester(
  request: RequestSummary,
  human: string | null,
): boolean {
  return human !== null && request.requester === `human:${human}`;
}

/** Only the requester sees actions; everyone else sees none. */
export function visibleActions(
  request: RequestSummary,
  human: string | null,
): readonly RequestAction[] {
  return isRequester(request, human) ? BY_STATE[request.state] : [];
}

export function isOlderDraft(shown: number | null, latest: number | null) {
  return shown !== null && latest !== null && shown !== latest;
}

/** An approval, a request for changes and an edit all name the latest draft, so an older draft tab leaves only the action that names no version. */
export function actionsOnDraft(
  actions: readonly RequestAction[],
  shown: number | null,
  latest: number | null,
): readonly RequestAction[] {
  if (!isOlderDraft(shown, latest)) return actions;
  return actions.filter((a) => a === "close");
}

export const OLDER_DRAFT_NOTE =
  "You are reading an older draft. Open the latest draft to approve it, ask for changes or edit it.";

const PHRASES: Record<RequestAction, string> = {
  approve: "approve it",
  request_changes: "ask for changes",
  edit: "edit it",
  close: "close it",
};

function sentenceList(items: readonly string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} or ${items.at(-1) ?? ""}`;
}

/** `You asked for RQ-4, so you can approve it, ask for changes, edit it or close it.`; while an action is pending, the last sentence says fold is saving it. */
export function decisionText(
  request: RequestSummary,
  actions: readonly RequestAction[],
  pending = false,
): string {
  const can = sentenceList(actions.map((a) => PHRASES[a]));
  const wait =
    request.state === "open" || request.state === "drafting"
      ? " The cards come once an orchestrator drafts them."
      : "";
  const last = pending
    ? "fold is saving your decision."
    : "Anyone else sees no actions.";
  return `You asked for ${request.id}, so you can ${can}.${wait} ${last}`;
}

/** The note under the versions: the requester learns that an approval names the version on screen, everyone else that each version needs its own approval. */
export function versionsNote(
  request: RequestSummary,
  human: string | null,
): Segments {
  return isRequester(request, human)
    ? [
        "Approval applies to the version you see. If a new version arrives first, fold asks you to look again.",
      ]
    : seg`${actorSegment(request.requester)} approves one version at a time. Each new version starts unapproved.`;
}

/** What a non-requester reads in place of the actions. */
export function viewerText(request: RequestSummary): Segments {
  return seg`Only ${actorSegment(request.requester)} can approve, ask for changes, edit or close.`;
}

/** The note under the pending action. */
export function pendingText(
  action: RequestAction,
  request: RequestSummary,
  cards: number,
): string {
  const off = "The other actions are off until it answers.";
  const v = `v${String(request.latest_version ?? 0)}`;
  switch (action) {
    case "approve":
      return `Waiting for fold to create ${String(cards)} ${cards === 1 ? "task" : "tasks"}. ${off}`;
    case "request_changes":
      return `Waiting for fold to save the comment on ${v}. ${off}`;
    case "edit":
      return `Waiting for fold to save v${String((request.latest_version ?? 0) + 1)}. The cards and edges stay locked until it answers.`;
    case "close":
      return `Waiting for fold to close ${request.id}. ${off}`;
  }
}

export interface ThreadTarget {
  /** The recipients the message names; undefined lets the daemon send it to the drafter. */
  to: string[] | undefined;
  /** Who the placeholder says the message reaches. */
  label: string;
}

/**
 * The drafter hears a message by default. Before a claim there is no drafter, so the message goes to the requester, or to every agent when the requester writes it.
 */
export function threadTarget(
  request: RequestSummary,
  human: string | null,
): ThreadTarget {
  if (request.drafter !== null) {
    return { to: undefined, label: atName(request.drafter) };
  }
  if (!isRequester(request, human)) {
    return { to: [request.requester], label: atName(request.requester) };
  }
  return { to: ["all"], label: "all agents" };
}

export type PanelKind = "editing" | "decide" | "viewer" | "approved" | "closed";

/** What the side panel shows: the outcome of a final request, the requester's actions or edit mode, or the note for everyone else. */
export function panelKind(
  request: RequestSummary,
  human: string | null,
  editing: boolean,
): PanelKind {
  if (request.state === "approved") return "approved";
  if (request.state === "closed") return "closed";
  if (!isRequester(request, human)) return "viewer";
  return editing ? "editing" : "decide";
}

/** Why the thread composer is off: the request is final. Null while anyone may write. */
export function threadClosedText(request: RequestSummary): string | null {
  if (request.state === "approved") {
    return "The request is approved, so the thread is closed for new messages.";
  }
  if (request.state === "closed") {
    return "The request is closed, so the thread is closed for new messages.";
  }
  return null;
}
