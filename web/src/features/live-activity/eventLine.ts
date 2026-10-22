import {
  Activity,
  Archive,
  ArchiveRestore,
  Bookmark,
  Bot,
  Check,
  CircleCheck,
  CircleDot,
  ClipboardCheck,
  FilePen,
  GitCommitHorizontal,
  GitMerge,
  GitPullRequest,
  Hand,
  ListChecks,
  MessageSquare,
  Milestone,
  Pause,
  Pencil,
  Hourglass,
  Play,
  RotateCcw,
  Tag,
  TriangleAlert,
  Undo2,
  Upload,
  X,
  type LucideIcon,
} from "lucide-react";
import type { DurableEvent } from "../../api/events";
import { codeData } from "../../format/codeText";
import { COLUMNS } from "../board/columns";

type EventType = DurableEvent["type"];

/** A stored event whose type may be one this build does not know. */
export type AnyEvent = Omit<DurableEvent, "type"> & { type: string };

export interface EventLine {
  icon: LucideIcon;
  /** The sentence after the actor. A command stands between backticks, and the event's data is escaped so that it never reads as a mark. */
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

/** A string of the event's data, escaped for a line that marks its commands. */
function text(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? codeData(value) : null;
}

/** A daemon text of the event's data that marks its commands between backticks itself, as it is. */
function marked(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

function count(value: unknown): number | null {
  return typeof value === "number" ? value : null;
}

/** The paths an event lists, joined, with the number the event left out; null when it lists none. */
function heldText(listed: unknown, total: unknown): string | null {
  const paths = Array.isArray(listed)
    ? listed
        .filter((path): path is string => typeof path === "string")
        .map(codeData)
    : [];
  if (paths.length === 0) {
    return null;
  }
  const more = (count(total) ?? paths.length) - paths.length;
  return more > 0
    ? `${paths.join(", ")} and ${String(more)} more`
    : paths.join(", ");
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

/** A queued integration is of one proposal or of a review batch. */
function queuedOf(event: DurableEvent): string {
  return text(event.data.batch) ?? proposalOf(event);
}

/** How a batch's integration ended, with each member it published as `P-3 as r18`. */
function batchIntegratedText(event: DurableEvent): string {
  const batch = batchOf(event);
  const list: unknown[] = Array.isArray(event.data.members)
    ? event.data.members
    : [];
  const members = list.flatMap((member) => {
    if (typeof member !== "object" || member === null) return [];
    const record = member as Record<string, unknown>;
    const proposal = text(record.proposal);
    const revision = text(record.revision);
    if (proposal === null) return [];
    return [revision === null ? proposal : `${proposal} as ${revision}`];
  });
  const published = members.join(", ");
  switch (text(event.data.outcome)) {
    case "together":
      return members.length === 0
        ? `integrated ${batch}`
        : `integrated ${batch} together: ${published}`;
    case "one_at_a_time":
      return members.length === 0
        ? `the checks of ${batch} failed together, and no member integrated on its own`
        : `the checks of ${batch} failed together, so its members integrated one at a time: ${published}`;
    default:
      return `${batch} had no member to integrate together`;
  }
}

function planOf(event: DurableEvent): string {
  return text(event.data.plan) ?? "a review plan";
}

function idsOf(value: unknown): string[] {
  const list: unknown[] = Array.isArray(value) ? value : [];
  return list.flatMap((item) => text(item) ?? []);
}

function planCreatedText(event: DurableEvent): string {
  const awaited = idsOf(event.data.awaited);
  return awaited.length === 0
    ? `made ${planOf(event)}`
    : `made ${planOf(event)}, waiting for ${awaited.join(", ")}`;
}

function planLeftText(event: DurableEvent): string {
  const reason = text(event.data.reason);
  const left = `${taskOf(event)} left ${planOf(event)}`;
  return reason === null ? left : `${left}: ${reason.replaceAll("_", " ")}`;
}

function planStartedText(event: DurableEvent): string {
  const batch = text(event.data.batch);
  return batch === null
    ? `started ${planOf(event)} for one reviewer`
    : `started ${planOf(event)} as ${batch}`;
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

/** ` with batch rb-3` for an integration that a review batch's members made together, else nothing. */
function withBatch(event: DurableEvent): string {
  const batch = text(event.data.batch);
  return batch === null ? "" : ` with batch ${batch}`;
}

/** A direct change of the main agent says so, because the user's word in the conversation approved it instead of a review. */
function integrationText(event: DurableEvent): string {
  const done =
    event.revision === null
      ? `integration succeeded${withBatch(event)}`
      : `integration succeeded${withBatch(event)} as ${event.revision}`;
  if (text(event.data.kind) !== "direct") {
    return done;
  }
  const changeset = text(event.data.changeset);
  const change =
    changeset === null ? "a direct change" : `direct change ${changeset}`;
  return event.revision === null
    ? `integrated ${change} with the user's OK`
    : `integrated ${change} with the user's OK as ${event.revision}`;
}

/** A worker's agent name. */
function workerOf(event: DurableEvent): string {
  return text(event.data.agent) ?? "a worker";
}

function bindRefusedText(event: DurableEvent): string {
  const agent = text(event.data.agent);
  const worker = text(event.data.worker);
  const from =
    worker === null ? "a worker's process" : `a process of ${worker}`;
  return agent === null
    ? `refused a session from ${from}`
    : `refused a session as ${agent} from ${from}`;
}

/** Stored events keep the role value they were written with, and `task_coder` there is the task worker role. */
const STORED_ROLES: ReadonlyMap<string, string> = new Map([
  ["task_coder", "task_worker"],
]);

/** A role with spaces, such as `task worker`. */
function roleText(role: string): string {
  return (STORED_ROLES.get(role) ?? role).replaceAll("_", " ");
}

function startedText(event: DurableEvent): string {
  const worker = workerOf(event);
  if (event.data.resumed === true) {
    return `resumed ${worker}`;
  }
  const role = text(event.data.role);
  return role === null
    ? `started ${worker}`
    : `started ${worker} as ${roleText(role)}`;
}

/** A task worker handed its proposal again, or a reviewer a new version of a proposal it reviews: `resumed` is false when its session was gone and it started in a new one. */
function resumedText(event: DurableEvent): string {
  const worker = workerOf(event);
  if (event.data.reason === "session_lost") {
    return `restarted ${worker} in a new session`;
  }
  if (event.data.reason === "new_version") {
    const version = count(event.data.version);
    const which =
      version === null
        ? `a new version of ${proposalOf(event)}`
        : `v${String(version)} of ${proposalOf(event)}`;
    return event.data.resumed === false
      ? `restarted ${worker} in a new session to review ${which}`
      : `resumed ${worker} to review ${which}`;
  }
  if (event.data.reason === "sent_back") {
    return `resumed ${worker} for the send-back of ${taskOf(event)}`;
  }
  const proposal = proposalOf(event);
  const why =
    event.data.reason === "conflict"
      ? `the conflict on ${proposal}`
      : event.data.reason === "checks"
        ? event.data.stage === "proposal"
          ? `the failed checks of ${taskOf(event)}`
          : `the failed integration checks of ${proposal}`
        : `changes to ${proposal}`;
  return event.data.resumed === false
    ? `restarted ${worker} in a new session for ${why}`
    : `resumed ${worker} for ${why}`;
}

/** The number of files a stash event names, as words. */
function filesText(event: DurableEvent): string {
  const files = count(event.data.file_count);
  if (files === null) {
    return "changes";
  }
  return files === 1 ? "1 file" : `${String(files)} files`;
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
  "session.bind_refused": { icon: TriangleAlert, text: bindRefusedText },
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
  "task.reported": {
    icon: ClipboardCheck,
    text: (e) => `reported ${taskOf(e)} finished for a lead check`,
  },
  "task.sent_back": {
    icon: Undo2,
    text: (e) => `sent ${taskOf(e)} back to its task worker`,
  },
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
  "task.hold_set": {
    icon: Hand,
    text: (e) => `put a blocker on ${taskOf(e)}`,
  },
  "task.hold_cleared": {
    icon: Check,
    text: (e) => `cleared the blocker of ${taskOf(e)}`,
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
  "proposal.human_edited": {
    icon: Pencil,
    text: (e) => {
      const version = count(e.data.version);
      return version === null
        ? `edited ${proposalOf(e)}`
        : `edited ${proposalOf(e)} as v${String(version)}`;
    },
  },
  "proposal.reviewer_fixed": {
    icon: Pencil,
    text: (e) => {
      const version = count(e.data.version);
      return version === null
        ? `fixed ${proposalOf(e)} in review`
        : `fixed ${proposalOf(e)} in review as v${String(version)}`;
    },
  },
  "proposal.review_again_requested": {
    icon: RotateCcw,
    text: (e) => {
      const version = count(e.data.version);
      return version === null
        ? `sent ${proposalOf(e)} for review again`
        : `sent v${String(version)} of ${proposalOf(e)} for review again`;
    },
  },
  "proposal.author_changed": {
    icon: GitPullRequest,
    text: (e) => {
      const from = text(e.data.from);
      return from === null
        ? `took over ${proposalOf(e)}`
        : `took over ${proposalOf(e)} from ${from}`;
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
        ? `integrated ${proposalOf(e)} into main${withBatch(e)}`
        : `integrated ${proposalOf(e)} into main${withBatch(e)} as ${e.revision}`,
    emphasis: true,
  },
  "review.submitted": { icon: Check, text: verdictText, look: verdictLook },
  "review.comment_added": {
    icon: MessageSquare,
    text: (e) => `commented on ${proposalOf(e)}`,
  },
  "review.batch_created": { icon: ListChecks, text: batchCreatedText },
  "review.scope_chosen": {
    icon: ListChecks,
    text: (e) => {
      const scope = text(e.data.scope);
      const version = count(e.data.version);
      const of =
        version === null
          ? proposalOf(e)
          : `${proposalOf(e)} v${String(version)}`;
      return scope === null
        ? `chose the review scope of ${of}`
        : `chose a ${scope} review of ${of}`;
    },
  },
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
    text: integrationText,
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
    text: fixed("the Git branch has commits fold did not write"),
  },
  "git.uncommitted_changes": {
    icon: TriangleAlert,
    text: (e) => {
      if (e.data.staged === true) {
        return "the Git index holds a staged change on a file main changed; run `git restore --staged` on it";
      }
      const held = heldText(e.data.held, e.data.held_count);
      return held === null
        ? "the project folder has changes that are not recorded; run `fld record`"
        : `the project folder waits on your changes to ${held} that are not recorded; run \`fld record\`, or take them back`;
    },
  },
  "git.sync_enabled": {
    icon: GitCommitHorizontal,
    text: fixed("turned Git sync on: every revision gets its Git commit"),
  },
  "git.head_moved": {
    icon: TriangleAlert,
    text: (e) => {
      const hint = marked(e.data.hint);
      return hint === null
        ? "the Git branch changed in a way fold did not make; the project folder waits"
        : `the Git branch changed in a way fold did not make; ${hint}`;
    },
  },
  "git.attention_cleared": {
    icon: GitCommitHorizontal,
    text: (e) =>
      e.data.kind === "outside_commits"
        ? "the Git branch has no commits fold did not write"
        : e.data.kind === "head_moved"
          ? "the Git branch is back where fold left it"
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
  "message.ask_closed": {
    icon: Check,
    text: (e) => {
      const message = text(e.data.message) ?? "an ask";
      return e.data.reason === "dismissed"
        ? `marked ${message} answered`
        : `recorded that ${message} was answered elsewhere`;
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
  "area.updated": {
    icon: Tag,
    text: (e) => `changed the profile of ${text(e.data.area) ?? "an area"}`,
  },
  "workers.limits_changed": {
    icon: Bot,
    text: () => "changed the worker limits",
  },
  "project.paused": {
    icon: Pause,
    text: () => "paused every agent of the project",
  },
  "project.resumed": {
    icon: Play,
    text: () => "resumed the agents of the project",
  },
  "worker.started": { icon: Bot, text: startedText },
  "worker.stopped": { icon: Bot, text: (e) => `stopped ${workerOf(e)}` },
  "worker.failed": {
    icon: TriangleAlert,
    text: (e) => `${workerOf(e)} failed`,
  },
  "worker.refreshed": {
    icon: Bot,
    text: (e) => {
      if (e.data.requested === true) {
        return `started ${workerOf(e)} again in a new session`;
      }
      return e.data.handoff === true
        ? `gave ${workerOf(e)} a fresh context with its hand-off note`
        : `gave ${workerOf(e)} a fresh context`;
    },
  },
  "worker.resumed": { icon: Bot, text: resumedText },
  "worker.permission_asked": {
    icon: Hand,
    text: (e) => {
      const tool = text(e.data.tool);
      return tool === null
        ? `${workerOf(e)} asked to use a tool`
        : `${workerOf(e)} asked to use ${tool}`;
    },
  },
  "worker.permission_answered": {
    icon: Hand,
    text: (e) => {
      const verb = e.data.behavior === "deny" ? "denied" : "allowed";
      const tool = text(e.data.tool) ?? "a tool";
      return `${verb} ${tool} for ${workerOf(e)}`;
    },
  },
  "human_change.recorded": {
    icon: GitMerge,
    emphasis: true,
    text: (e) =>
      e.revision === null
        ? "recorded a change of the project folder"
        : `recorded a change of the project folder as ${e.revision}`,
  },
  "human_change.conflicted": {
    icon: TriangleAlert,
    text: fixed(
      "recorded a change of the project folder that conflicts with main; main did not move",
    ),
  },
  "human_change.resolved": {
    icon: GitMerge,
    text: (e) =>
      e.revision === null
        ? "resolved the change of the project folder"
        : `resolved the change of the project folder in ${e.revision}`,
  },
  "human_change.withdrawn": {
    icon: Undo2,
    text: fixed(
      "took back the change of the project folder that waited for the main agent",
    ),
  },
  "stash.pushed": {
    icon: Archive,
    text: (e) => `stashed ${filesText(e)} of the project folder`,
  },
  "stash.applied": {
    icon: ArchiveRestore,
    text: (e) => `unstashed ${filesText(e)} into the project folder`,
  },
  "stash.conflicted": {
    icon: TriangleAlert,
    text: (e) => {
      const paths = heldText(e.data.conflicted, e.data.conflicted_count);
      return paths === null
        ? "unstashed changes that conflict with the project folder; the entry stays"
        : `unstashed changes that conflict with the project folder in ${paths}; the entry stays`;
    },
  },
  "stash.dropped": {
    icon: Check,
    text: (e) => {
      // fold drops the entry itself when a Git commit put the markers on main, and names the task that fixes them.
      const task = text(e.data.task);
      return task === null
        ? "fixed the conflict of the unstash; the stash entry is dropped"
        : `recorded the conflict of the unstash on main; ${task} fixes it`;
    },
  },
  "checks.started": {
    icon: ListChecks,
    text: (e) => `started the checks of ${taskOf(e)}`,
  },
  "checks.finished": {
    icon: ListChecks,
    text: (e) => {
      const state = text(e.data.state);
      return state === null
        ? `ended the checks of ${taskOf(e)}`
        : `ended the checks of ${taskOf(e)}: ${state.replaceAll("_", " ")}`;
    },
  },
  "integration.queued": {
    icon: ListChecks,
    text: (e) => `queued the integration of ${queuedOf(e)}`,
  },
  "integration.unqueued": {
    icon: ListChecks,
    text: (e) => `took the integration of ${queuedOf(e)} out of the queue`,
  },
  "integration.held": {
    icon: TriangleAlert,
    text: (e) =>
      e.revision === null
        ? "held the integrations: fold-checks.toml on main is not valid"
        : `held the integrations: fold-checks.toml on ${e.revision} is not valid`,
  },
  "worker.model_unreachable": {
    icon: TriangleAlert,
    text: (e) => `${workerOf(e)} cannot reach the model`,
  },
  "review.plan_created": { icon: Hourglass, text: planCreatedText },
  "review.plan_changed": {
    icon: Hourglass,
    text: (e) => `changed ${planOf(e)}`,
  },
  "review.plan_joined": {
    icon: ListChecks,
    text: (e) => `${proposalOf(e)} joined ${planOf(e)}`,
  },
  "review.plan_left": { icon: X, text: planLeftText },
  "review.plan_wait_ended": {
    icon: Hourglass,
    text: (e) => `the wait of ${planOf(e)} ended`,
  },
  "review.plan_started": { icon: Play, text: planStartedText },
  "review.plan_ended": { icon: X, text: (e) => `ended ${planOf(e)}` },
  "review.batch_integrated": {
    icon: GitMerge,
    text: batchIntegratedText,
    emphasis: true,
  },
  "review.batch_waiting": {
    icon: Hourglass,
    text: (e) => `${batchOf(e)} waits for a verdict on ${proposalOf(e)}`,
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
