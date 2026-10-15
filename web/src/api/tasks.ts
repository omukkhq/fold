import { queryOptions } from "@tanstack/react-query";
import type {
  AttentionItem,
  BoardActivity,
  BoardLabel,
  ProposalState,
  TaskStatus,
} from "./board";
import { ApiError } from "./errors";
import { fromFixture } from "./fixture";
import { keys } from "./keys";
import type { components } from "./schema";

export type CheckpointSummary = components["schemas"]["CheckpointSummary"];
export type LiveWorkspace = components["schemas"]["LiveWorkspace"];
export type DurableEventType = components["schemas"]["DurableEventType"];

export interface StepItem {
  /** The daemon's id for the item; the web only passes it back. */
  id: string;
  kind: "step";
  text: string;
  done: boolean;
}

/** A linked item names its child; fold ticks it when the child reaches Done, so `done` is never set by hand. */
export interface LinkedItem {
  id: string;
  kind: "linked";
  /** @example T-13 */
  task: string;
  title: string;
  status: TaskStatus;
  done: boolean;
}

export type ChecklistItem = StepItem | LinkedItem;

export interface Checklist {
  /** The first checklist is `Subtasks`. */
  name: string;
  items: ChecklistItem[];
}

/** `needs`: this task needs `task`. `needed_by`: `task` needs this task. */
export type EdgeDirection = "needs" | "needed_by";

/** An edge added through a linked checklist item has the origin `checklist` and goes away only with the item. */
export type EdgeOrigin = "direct" | "checklist";

export interface TaskEdge {
  /** The task at the other end of the edge. */
  task: string;
  title: string;
  status: TaskStatus;
  direction: EdgeDirection;
  origin: EdgeOrigin;
}

export interface TaskClaim {
  kind: "implementation" | "review";
  /** The worker, `agent` or `agent/worker`. */
  holder: string;
  /** UTC milliseconds. */
  claimed_at: number;
  /** UTC milliseconds of the last lease renewal. */
  renewed_at: number;
  /** The daemon marks a claim stale when its lease was not renewed in time. */
  stale: boolean;
}

export interface TaskWorkspace {
  /** @example ws-9 */
  id: string;
  /** Relative to the project root. */
  directory: string;
  /** The revision the workspace is based on, such as `r7`. */
  base: string;
  /** The main head, such as `r8`. */
  main: string;
  /** How many revisions main moved since `base`. */
  behind: number;
  /** The live state when the task was read; a newer frame from the event stream replaces it. */
  live: LiveWorkspace | null;
}

export interface TaskChangeset {
  /** @example C-18 */
  id: string;
  message: string;
  /** UTC milliseconds. */
  created_at: number;
}

export type ReviewVerdict =
  "comment" | "request_changes" | "approve" | "reject";

export interface TaskReview {
  reviewer: string;
  verdict: ReviewVerdict;
  /** The proposal version the review belongs to. */
  version: number;
  /** UTC milliseconds. */
  at: number;
}

export interface TaskProposal {
  /** @example P-3 */
  id: string;
  state: ProposalState;
  /** The current version number. */
  version: number;
  reviews: TaskReview[];
}

export interface TaskIntegration {
  /** The revision the integration published, such as `r8`. */
  revision: string;
  proposal: string;
  /** The integrating worker or human. */
  by: string;
  /** UTC milliseconds. */
  at: number;
}

export interface TaskMessage {
  /** @example m-210 */
  id: string;
  /** The id of the first message of the thread. */
  thread: string;
  from: string;
  to: string;
  body: string;
  /** UTC milliseconds. */
  at: number;
}

export interface TaskHumanReview {
  /** The named human whose approval integration waits for; null when any human may approve. */
  reviewer: string | null;
  reason: string;
  /** The worker or human that turned the mark on. */
  set_by: string;
  /** UTC milliseconds. */
  set_at: number;
}

export interface TaskEvent {
  type: DurableEventType;
  actor: string;
  /** UTC milliseconds. */
  at: number;
}

export interface TaskDetail {
  /** @example T-12 */
  task: string;
  title: string;
  /** Markdown. */
  description: string;
  status: TaskStatus;
  /** The daemon's explanation of the calculated status. */
  status_reason: string;
  blocked: boolean;
  labels: BoardLabel[];
  checklists: Checklist[];
  /** Prerequisites and dependents together; `direction` tells them apart. */
  dependencies: TaskEdge[];
  claims: TaskClaim[];
  activity: BoardActivity | null;
  attention: AttentionItem[];
  workspace: TaskWorkspace | null;
  /** Newest first. */
  checkpoints: CheckpointSummary[];
  /** Newest first. */
  changesets: TaskChangeset[];
  proposal: TaskProposal | null;
  integration: TaskIntegration | null;
  /** Oldest first. */
  messages: TaskMessage[];
  human_review: TaskHumanReview | null;
  /** Whether the viewer may turn human review on or off; the daemon decides. */
  can_change_human_review: boolean;
  /** Newest first. */
  history: TaskEvent[];
}

/** Whom the feedback box can address besides the claim holder. */
export interface MessageTargets {
  agents: string[];
  tags: string[];
}

export type FeedbackRecipient =
  | { kind: "holder" }
  | { kind: "agent"; name: string }
  | { kind: "tag"; name: string };

export type NewChecklistItem =
  { kind: "step"; text: string } | { kind: "linked"; task: string };

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/tasks")
  : undefined;

type FixtureModule = typeof import("../../fixtures/tasks");

function taskNotFound(task: string): ApiError {
  return new ApiError("task_not_found", `task ${task} does not exist`);
}

/** Rejects with `task_not_found` for a task the project does not have. */
export async function getTask(slug: string, task: string): Promise<TaskDetail> {
  const found = await fromFixture(fixture, (m) => m.task(slug, task), null);
  if (found === null) {
    throw taskNotFound(task);
  }
  return found;
}

export async function getMessageTargets(slug: string): Promise<MessageTargets> {
  return fromFixture(fixture, (m) => m.messageTargets(slug), {
    agents: [],
    tags: [],
  });
}

/** A production build has no fixture and no task, so every write is refused the way a missing task is. */
async function write(
  task: string,
  apply: (m: FixtureModule) => void,
): Promise<void> {
  if (!fixture) {
    throw taskNotFound(task);
  }
  await fromFixture(
    fixture,
    (m) => {
      apply(m);
      return null;
    },
    null,
  );
}

/** Answers the new task's public id. */
export async function createTask(
  slug: string,
  input: { title: string; description: string },
): Promise<string> {
  if (!fixture) {
    throw new ApiError("project_unavailable", `project ${slug} is unavailable`);
  }
  return fromFixture(fixture, (m) => m.createTask(slug, input), "");
}

export function setTitle(slug: string, task: string, title: string) {
  return write(task, (m) => {
    m.setTitle(slug, task, title);
  });
}

export function setDescription(
  slug: string,
  task: string,
  description: string,
) {
  return write(task, (m) => {
    m.setDescription(slug, task, description);
  });
}

export function addLabel(slug: string, task: string, label: BoardLabel) {
  return write(task, (m) => {
    m.addLabel(slug, task, label);
  });
}

export function removeLabel(slug: string, task: string, name: string) {
  return write(task, (m) => {
    m.removeLabel(slug, task, name);
  });
}

/** A linked item also adds the edge "this task needs the child", so it can be refused with `dependency_cycle`. */
export function addChecklistItem(
  slug: string,
  task: string,
  checklist: string,
  item: NewChecklistItem,
) {
  return write(task, (m) => {
    m.addChecklistItem(slug, task, checklist, item);
  });
}

export function renameChecklistItem(
  slug: string,
  task: string,
  item: string,
  text: string,
) {
  return write(task, (m) => {
    m.renameChecklistItem(slug, task, item, text);
  });
}

/** Moves the item to `position`, counted from 0 in its checklist. */
export function moveChecklistItem(
  slug: string,
  task: string,
  item: string,
  position: number,
) {
  return write(task, (m) => {
    m.moveChecklistItem(slug, task, item, position);
  });
}

/** Removing a linked item also removes its edge. */
export function removeChecklistItem(slug: string, task: string, item: string) {
  return write(task, (m) => {
    m.removeChecklistItem(slug, task, item);
  });
}

export function tickChecklistItem(
  slug: string,
  task: string,
  item: string,
  done: boolean,
) {
  return write(task, (m) => {
    m.tickChecklistItem(slug, task, item, done);
  });
}

/** Refused with `dependency_cycle` and the cycle in `details.cycle` when the edge would close a cycle. */
export function addDependency(slug: string, task: string, needs: string) {
  return write(task, (m) => {
    m.addDependency(slug, task, needs);
  });
}

export function removeDependency(slug: string, task: string, needs: string) {
  return write(task, (m) => {
    m.removeDependency(slug, task, needs);
  });
}

export function setHumanReview(
  slug: string,
  task: string,
  mark: { reviewer: string | null; reason: string },
) {
  return write(task, (m) => {
    m.setHumanReview(slug, task, mark);
  });
}

export function clearHumanReview(slug: string, task: string) {
  return write(task, (m) => {
    m.clearHumanReview(slug, task);
  });
}

/** The message is from the signed-in human and linked to the task. */
export function sendFeedback(
  slug: string,
  task: string,
  feedback: { body: string; to: FeedbackRecipient },
) {
  return write(task, (m) => {
    m.sendFeedback(slug, task, feedback);
  });
}

export const taskQuery = (slug: string, task: string) =>
  queryOptions({
    queryKey: keys.task(slug, task),
    queryFn: () => getTask(slug, task),
  });

export const messageTargetsQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.messageTargets(slug),
    queryFn: () => getMessageTargets(slug),
  });
