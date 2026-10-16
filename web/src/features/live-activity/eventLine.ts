import {
  Activity,
  Bookmark,
  Bot,
  Check,
  CircleCheck,
  CircleDot,
  FilePen,
  GitCommitHorizontal,
  GitMerge,
  GitPullRequest,
  Hand,
  ListChecks,
  MessageSquare,
  Milestone,
  Play,
  Tag,
  TriangleAlert,
  Undo2,
  Upload,
  X,
  type LucideIcon,
} from "lucide-react";
import type { DurableEvent } from "../../api/events";
import { COLUMNS } from "../board/columns";

type EventType = DurableEvent["type"];

/** A stored event whose type may be one this build does not know. */
export type AnyEvent = Omit<DurableEvent, "type"> & { type: string };

export interface EventLine {
  icon: LucideIcon;
  /** The sentence after the actor, built from ids and counts only. */
  text: string;
  /** Integrations, approvals and main moves read in the full text colour. */
  emphasis: boolean;
}

interface Look {
  icon: LucideIcon;
  emphasis: boolean;
}

interface Spec {
  icon: LucideIcon;
  text: (event: DurableEvent) => string;
  emphasis?: boolean;
  /** Replaces `icon` and `emphasis` for a type whose look depends on its data. */
  look?: (event: DurableEvent) => Look;
}

function text(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

function count(value: unknown): number | null {
  return typeof value === "number" ? value : null;
}

function on(event: DurableEvent): string {
  return event.task === null ? "" : ` on ${event.task}`;
}

function proposalOf(event: DurableEvent): string {
  return event.proposal ?? "a proposal";
}

function taskOf(event: DurableEvent): string {
  return event.task ?? "a task";
}

function workspaceOf(event: DurableEvent): string {
  return event.workspace ?? "a workspace";
}

function requestOf(event: DurableEvent): string {
  const request = text(event.data.request) ?? "a request";
  const version = count(event.data.version);
  return version === null ? request : `${request} v${String(version)}`;
}

function batchOf(event: DurableEvent): string {
  return text(event.data.batch) ?? "a review batch";
}

function batchCreatedText(event: DurableEvent): string {
  const members: unknown[] = Array.isArray(event.data.members)
    ? event.data.members
    : [];
  return members.length === 0
    ? `started ${batchOf(event)}`
    : `batched ${String(members.length)} proposals for review as ${batchOf(event)}`;
}

function approvedText(event: DurableEvent): string {
  const list: unknown[] = Array.isArray(event.data.tasks)
    ? event.data.tasks
    : [];
  const tasks = list.flatMap((task) => text(task) ?? []);
  const request = text(event.data.request) ?? "a request";
  return tasks.length === 0
    ? `approved ${request}`
    : `approved ${request} as ${tasks.join(", ")}`;
}

/** A worker's agent name, and its role with spaces, such as `task coder`. */
function workerOf(event: DurableEvent): string {
  return text(event.data.agent) ?? "a worker";
}

function startedText(event: DurableEvent): string {
  const worker = workerOf(event);
  if (event.data.resumed === true) {
    return `resumed ${worker}`;
  }
  const role = text(event.data.role);
  return role === null
    ? `started ${worker}`
    : `started ${worker} as ${role.replaceAll("_", " ")}`;
}

function fixed(line: string): (event: DurableEvent) => string {
  return () => line;
}

function versioned(event: DurableEvent): string {
  const version = count(event.data.version);
  return version === null
    ? proposalOf(event)
    : `${proposalOf(event)} v${String(version)}`;
}

function verdictText(event: DurableEvent): string {
  const proposal = versioned(event);
  switch (text(event.data.verdict)) {
    case "approve":
      return `approved ${proposal}`;
    case "request_changes":
      return `requested changes on ${proposal}`;
    case "reject":
      return `rejected ${proposal}`;
    default:
      return `commented on ${proposal}`;
  }
}

/** Only an approval reads in the full text colour; a refusal and a comment keep the muted line. */
function verdictLook(event: DurableEvent): Look {
  switch (text(event.data.verdict)) {
    case "approve":
      return { icon: Check, emphasis: true };
    case "request_changes":
    case "reject":
      return { icon: X, emphasis: false };
    default:
      return { icon: MessageSquare, emphasis: false };
  }
}

/** A move to an earlier board column reads "back to", the way a task returns from Review to In Progress. */
function statusText(event: DurableEvent): string {
  const order = (value: unknown) =>
    COLUMNS.findIndex((column) => column.status === value);
  const to = order(event.data.to);
  const column = COLUMNS[to];
  if (column === undefined) {
    return `${taskOf(event)} changed status`;
  }
  const verb = order(event.data.from) > to ? "back to" : "moved to";
  return `${taskOf(event)} ${verb} ${column.name}`;
}

function isReview(event: DurableEvent): boolean {
  return text(event.data.kind) === "review";
}

/** One entry for every stored event type of the generated schema; the type makes a missing entry a compile error. */
const SPECS: Record<EventType, Spec> = {
  "agent.registered": { icon: Bot, text: fixed("registered as an agent") },
  "worker.registered": { icon: Bot, text: fixed("registered as a worker") },
  "session.started": { icon: Play, text: fixed("started a session") },
  "session.ended": { icon: Play, text: fixed("ended a session") },
  "agent.tag_added": { icon: Tag, text: fixed("added an agent tag") },
  "agent.tag_removed": { icon: Tag, text: fixed("removed an agent tag") },
  "task.created": {
    icon: CircleDot,
    text: (e) => `created ${taskOf(e)}`,
  },
  "task.updated": {
    icon: FilePen,
    text: (e) => `updated ${taskOf(e)}`,
  },
  "task.deleted": {
    icon: X,
    text: (e) => `deleted ${taskOf(e)}`,
  },
  "task.archived": {
    icon: X,
    text: (e) => `archived ${taskOf(e)}`,
  },
  "task.completed": {
    icon: CircleCheck,
    text: (e) => `${taskOf(e)} is Done`,
  },
  "task.status_changed": { icon: Undo2, text: statusText },
  "task.blocked_changed": {
    icon: Hand,
    text: (e) =>
      e.data.blocked === true
        ? `${taskOf(e)} is blocked`
        : `${taskOf(e)} is unblocked`,
  },
  "task.human_review_set": {
    icon: Hand,
    text: (e) => `${taskOf(e)} waits for a human review`,
  },
  "task.human_review_cleared": {
    icon: Check,
    text: (e) => `${taskOf(e)} no longer waits for a human review`,
  },
  "label.created": { icon: Tag, text: fixed("created a label") },
  "dependency.added": {
    icon: ListChecks,
    text: (e) => {
      const needs = text(e.data.needs);
      return needs === null
        ? `added a dependency${on(e)}`
        : `${taskOf(e)} needs ${needs}`;
    },
  },
  "dependency.removed": {
    icon: ListChecks,
    text: (e) => {
      const needs = text(e.data.needs);
      return needs === null
        ? `removed a dependency${on(e)}`
        : `${taskOf(e)} no longer needs ${needs}`;
    },
  },
  "checklist.created": {
    icon: ListChecks,
    text: (e) => `added a checklist${on(e)}`,
  },
  "checklist.item_added": {
    icon: ListChecks,
    text: (e) => `added a checklist item${on(e)}`,
  },
  "checklist.item_updated": {
    icon: ListChecks,
    text: (e) => `changed a checklist item${on(e)}`,
  },
  "checklist.item_done": {
    icon: Check,
    text: (e) => `finished a checklist item${on(e)}`,
  },
  "checklist.item_removed": {
    icon: ListChecks,
    text: (e) => `removed a checklist item${on(e)}`,
  },
  "claim.acquired": {
    icon: Hand,
    text: (e) =>
      isReview(e)
        ? `claimed the review of ${proposalOf(e)}`
        : `claimed ${taskOf(e)}`,
  },
  "claim.released": {
    icon: Hand,
    text: (e) =>
      isReview(e)
        ? `released the review of ${proposalOf(e)}`
        : `released ${taskOf(e)}`,
  },
  "claim.taken_over": {
    icon: Hand,
    text: (e) =>
      isReview(e)
        ? `took over the review of ${proposalOf(e)}`
        : `took over ${taskOf(e)}`,
  },
  "claim.stale": {
    icon: TriangleAlert,
    text: (e) => `the claim on ${taskOf(e)} is stale`,
  },
  "workspace.created": {
    icon: FilePen,
    text: (e) =>
      isReview(e)
        ? `checked out ${versioned(e)} in ${workspaceOf(e)}`
        : `created ${workspaceOf(e)}${on(e)}`,
  },
  "workspace.adopted": {
    icon: FilePen,
    text: (e) => `adopted ${workspaceOf(e)}${on(e)}`,
  },
  "workspace.abandoned": {
    icon: X,
    text: (e) => `abandoned ${workspaceOf(e)}${on(e)}`,
  },
  "workspace.archived": {
    icon: X,
    text: (e) => `archived ${workspaceOf(e)}${on(e)}`,
  },
  "workspace.synced": {
    icon: FilePen,
    text: (e) => `synced ${workspaceOf(e)} to main`,
  },
  "workspace.missing": {
    icon: TriangleAlert,
    text: (e) => `${workspaceOf(e)} is missing`,
  },
  "workspace.recovered": {
    icon: FilePen,
    text: (e) => `${workspaceOf(e)} is back`,
  },
  "workspace.watch_degraded": {
    icon: TriangleAlert,
    text: (e) => `${workspaceOf(e)} refreshes on a periodic scan`,
  },
  "checkpoint.recorded": {
    icon: Bookmark,
    text: (e) => {
      const checkpoint = text(e.data.checkpoint) ?? "a checkpoint";
      return text(e.data.kind) === "auto"
        ? `auto checkpoint ${checkpoint}${on(e)}`
        : `recorded ${checkpoint}${on(e)}`;
    },
  },
  "checkpoint.restored": {
    icon: Undo2,
    text: (e) => {
      const checkpoint = text(e.data.checkpoint);
      return `restored ${checkpoint ?? "a checkpoint"} in ${workspaceOf(e)}`;
    },
  },
  "checkpoint.pruned": {
    icon: Bookmark,
    text: (e) => {
      const checkpoint = text(e.data.checkpoint) ?? "a checkpoint";
      return e.workspace === null
        ? `pruned ${checkpoint}`
        : `pruned ${checkpoint} in ${e.workspace}`;
    },
  },
  "changeset.recorded": {
    icon: GitCommitHorizontal,
    text: (e) => {
      const changeset = text(e.data.changeset);
      return `recorded ${changeset ?? "a changeset"}${on(e)}`;
    },
  },
  "proposal.opened": {
    icon: GitPullRequest,
    text: (e) =>
      e.task === null
        ? `opened ${versioned(e)}`
        : `opened ${versioned(e)} for ${e.task}`,
  },
  "proposal.version_added": {
    icon: GitPullRequest,
    text: (e) => {
      const version = count(e.data.version);
      return version === null
        ? `added a version to ${proposalOf(e)}`
        : `added v${String(version)} to ${proposalOf(e)}`;
    },
  },
  "proposal.closed": {
    icon: X,
    text: (e) => `closed ${proposalOf(e)}`,
  },
  "proposal.integrated": {
    icon: GitMerge,
    text: (e) =>
      e.revision === null
        ? `integrated ${proposalOf(e)} into main`
        : `integrated ${proposalOf(e)} into main as ${e.revision}`,
    emphasis: true,
  },
  "review.submitted": { icon: Check, text: verdictText, look: verdictLook },
  "review.comment_added": {
    icon: MessageSquare,
    text: (e) => `commented on ${proposalOf(e)}`,
  },
  "review.batch_created": { icon: ListChecks, text: batchCreatedText },
  "review.batch_submitted": {
    icon: Check,
    text: (e) => `reviewed ${batchOf(e)}`,
  },
  "review.batch_closed": {
    icon: X,
    text: (e) => `closed ${batchOf(e)}`,
  },
  "integration.succeeded": {
    icon: GitMerge,
    text: (e) =>
      e.revision === null
        ? "integration succeeded"
        : `integration succeeded as ${e.revision}`,
    emphasis: true,
  },
  "integration.conflicted": {
    icon: TriangleAlert,
    text: (e) => `integration of ${proposalOf(e)} conflicted`,
  },
  "integration.refused": {
    icon: TriangleAlert,
    text: (e) => `integration of ${proposalOf(e)} was refused`,
  },
  "main.advanced": {
    icon: Milestone,
    text: (e) =>
      e.revision === null ? "main advanced" : `main advanced to ${e.revision}`,
    emphasis: true,
  },
  "remote.pushed": { icon: Upload, text: fixed("pushed main to the remote") },
  "remote.push_failed": {
    icon: TriangleAlert,
    text: fixed("push to the remote failed"),
  },
  "remote.pulled": { icon: Upload, text: fixed("pulled from the remote") },
  "remote.pull_conflicted": {
    icon: TriangleAlert,
    text: fixed("pull from the remote conflicted"),
  },
  "git.commit_failed": {
    icon: TriangleAlert,
    text: (e) =>
      e.revision === null
        ? "writing the Git commit failed"
        : `writing the Git commit of ${e.revision} failed`,
  },
  "git.committed": {
    icon: GitCommitHorizontal,
    text: (e) =>
      e.revision === null
        ? "wrote a Git commit"
        : `wrote the Git commit of ${e.revision}`,
  },
  "git.outside_commits": {
    icon: TriangleAlert,
    text: fixed("the Git branch has commits fold did not write; run fld pull"),
  },
  "git.uncommitted_changes": {
    icon: TriangleAlert,
    text: fixed(
      "the project folder has changes that are not committed; commit or stash them",
    ),
  },
  "git.attention_cleared": {
    icon: GitCommitHorizontal,
    text: (e) =>
      e.data.kind === "outside_commits"
        ? "the Git branch has no commits fold did not write"
        : "the project folder follows main again",
  },
  "message.sent": {
    icon: MessageSquare,
    text: (e) => {
      const message = text(e.data.message);
      return `sent ${message ?? "a message"}${on(e)}`;
    },
  },
  "message.acknowledged": {
    icon: Check,
    text: (e) => {
      const list: unknown[] = Array.isArray(e.data.ids) ? e.data.ids : [];
      const ids = list.flatMap((id) => text(id) ?? []);
      if (ids.length > 1) {
        return `acknowledged ${String(ids.length)} messages`;
      }
      return `acknowledged ${ids[0] ?? "a message"}`;
    },
  },
  "request.created": {
    icon: MessageSquare,
    text: (e) => `created ${requestOf(e)}`,
  },
  "request.claimed": {
    icon: Hand,
    text: (e) =>
      e.data.taken_over === true
        ? `took over ${requestOf(e)}`
        : `claimed ${requestOf(e)}`,
  },
  "request.released": {
    icon: Undo2,
    text: (e) => `released ${requestOf(e)}`,
  },
  "request.draft_submitted": {
    icon: FilePen,
    text: (e) => `drafted ${requestOf(e)}`,
  },
  "request.changes_requested": {
    icon: X,
    text: (e) => `requested changes on ${requestOf(e)}`,
  },
  "request.approved": { icon: Check, text: approvedText, emphasis: true },
  "request.closed": {
    icon: X,
    text: (e) => `closed ${requestOf(e)}`,
  },
  "area.defined": {
    icon: Tag,
    text: (e) => {
      const area = text(e.data.area) ?? "an area";
      const label = text(e.data.label);
      return label === null
        ? `defined ${area}`
        : `defined ${area} over label ${label}`;
    },
  },
  "worker.started": { icon: Bot, text: startedText },
  "worker.stopped": { icon: Bot, text: (e) => `stopped ${workerOf(e)}` },
  "worker.failed": {
    icon: TriangleAlert,
    text: (e) => `${workerOf(e)} failed`,
  },
};

/** Every stored event type this build writes a line for. */
export const EVENT_TYPES = Object.keys(SPECS) as EventType[];

function isKnown(type: string): type is EventType {
  return Object.hasOwn(SPECS, type);
}

/** A type the daemon added after this build shows its name, because the daemon can be newer than the web build. */
export function eventLine(event: AnyEvent): EventLine {
  const { type } = event;
  if (!isKnown(type)) {
    return { icon: Activity, text: `event ${type}`, emphasis: false };
  }
  const spec = SPECS[type];
  const known = { ...event, type };
  const look = spec.look?.(known) ?? {
    icon: spec.icon,
    emphasis: spec.emphasis ?? false,
  };
  return { ...look, text: spec.text(known) };
}
