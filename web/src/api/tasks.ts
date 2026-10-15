import { queryOptions } from "@tanstack/react-query";
import type { BoardActivity, BoardLabel, ProposalState } from "./board";
import { api } from "./client";
import { ApiError } from "./errors";
import { keys } from "./keys";
import { expectOk, unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type CheckpointSummary = Schemas["CheckpointSummary"];
export type LiveWorkspace = Schemas["LiveWorkspace"];

export type Checklist = Schemas["Checklist"];
export type ChecklistItem = Schemas["ChecklistItem"];
export type StepItem = Extract<ChecklistItem, { kind: "step" }>;
export type LinkedItem = Extract<ChecklistItem, { kind: "linked" }>;
export type TaskEdge = Schemas["TaskEdge"];
export type TaskClaim = Schemas["TaskClaim"];
export type EdgeDirection = Schemas["EdgeDirection"];
export type EdgeOrigin = Schemas["EdgeOrigin"];

export type DurableEventType = Schemas["DurableEventType"];
export type TaskMessage = Schemas["MessageView"];

export interface TaskWorkspaceInfo {
  /** Relative to the project root. */
  directory: string;
  /** The revision the workspace is based on, such as `r7`. */
  base: string;
  /** The main head, such as `r8`. */
  main: string;
  /** How many revisions main moved since `base`. */
  behind: number;
}

export interface TaskChangeset {
  /** @example C-18 */
  id: string;
  message: string;
  /** UTC milliseconds. */
  created_at: number;
}

export type ReviewVerdict =
  | "comment"
  | "request_changes"
  | "approve"
  | "reject";

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
  version: number;
  reviews: TaskReview[];
}

export interface TaskIntegration {
  /** The revision the integration published, such as `r8`. */
  revision: string;
  proposal: string;
  by: string;
  /** UTC milliseconds. */
  at: number;
}

export interface TaskHumanReviewNote {
  reason: string;
  /** The worker or human that turned the mark on. */
  set_by: string;
  /** UTC milliseconds. */
  set_at: number;
}

/** A row of the task's history: a stored event as the event stream and the activity list carry it. */
export type TaskEvent = Pick<
  Schemas["DurableEvent"],
  "seq" | "type" | "actor" | "created_at"
>;

/**
 * Fields the dialog can draw that the task read does not carry. Each one is optional: an absent field draws the
 * section's empty state, or leaves the section out where its frame has none.
 */
export interface TaskExtras {
  /** The daemon's explanation of the calculated status. */
  status_reason: string;
  activity: BoardActivity | null;
  workspace_info: TaskWorkspaceInfo;
  /** Newest first. */
  checkpoints: CheckpointSummary[];
  /** Newest first. */
  changesets: TaskChangeset[];
  proposal: TaskProposal | null;
  integration: TaskIntegration | null;
  human_review_note: TaskHumanReviewNote;
  /** Whether the viewer may turn human review on or off; the daemon decides. */
  can_change_human_review: boolean;
  /** Newest first. */
  history: TaskEvent[];
}

export type TaskDetail = Schemas["TaskView"] & Partial<TaskExtras>;

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
  | { kind: "step"; text: string }
  | { kind: "linked"; task: string };

/** The `to` list of a feedback request. Leaving it out sends the feedback to the task's claim holder. */
export function feedbackTo(to: FeedbackRecipient): string[] | undefined {
  switch (to.kind) {
    case "holder":
      return undefined;
    case "agent":
      return [to.name];
    case "tag":
      return [`tag:${to.name}`];
  }
}

export function getTask(slug: string, task: string): Promise<TaskDetail> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/tasks/{id}", {
      params: { path: { slug, id: task } },
    }),
  );
}

/** The agents and workers a human can write to, and every tag in use. */
export async function getMessageTargets(slug: string): Promise<MessageTargets> {
  const { agents } = await unwrap(
    api.GET("/api/v1/projects/{slug}/agents", {
      params: { path: { slug } },
    }),
  );
  const names = agents.flatMap((agent) => [
    agent.name,
    ...agent.workers
      .filter((worker) => worker.parent !== null)
      .map((worker) => worker.name),
  ]);
  const tags = [...new Set(agents.flatMap((agent) => agent.tags))].sort();
  return { agents: names, tags };
}

/** Answers the new task's public id. */
export async function createTask(
  slug: string,
  input: { title: string; description: string },
): Promise<string> {
  const created = await unwrap(
    api.POST("/api/v1/projects/{slug}/tasks", {
      params: { path: { slug } },
      body: input,
    }),
  );
  return created.task;
}

async function update(
  slug: string,
  task: string,
  body: Schemas["UpdateTaskBody"],
): Promise<void> {
  await unwrap(
    api.PATCH("/api/v1/projects/{slug}/tasks/{id}", {
      params: { path: { slug, id: task } },
      body,
    }),
  );
}

export function setTitle(slug: string, task: string, title: string) {
  return update(slug, task, { title });
}

export function setDescription(
  slug: string,
  task: string,
  description: string,
) {
  return update(slug, task, { description });
}

export async function listLabels(slug: string): Promise<BoardLabel[]> {
  const { labels } = await unwrap(
    api.GET("/api/v1/projects/{slug}/labels", {
      params: { path: { slug } },
    }),
  );
  return labels;
}

export function createLabel(
  slug: string,
  label: BoardLabel,
): Promise<BoardLabel> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/labels", {
      params: { path: { slug } },
      body: label,
    }),
  );
}

/** Creates the label and puts it on the task. A refused creation, such as `label_exists`, leaves the task as it was. */
export async function createLabelOn(
  slug: string,
  task: string,
  label: BoardLabel,
): Promise<void> {
  await createLabel(slug, label);
  await attachLabel(slug, task, label.name);
}

/** The route takes the whole label set, so the set is read again first: a label another writer added meanwhile stays. */
async function changeLabels(
  slug: string,
  task: string,
  change: (names: string[]) => string[],
): Promise<void> {
  const current = await getTask(slug, task);
  const names = current.labels.map((label) => label.name);
  await update(slug, task, { labels: change(names) });
}

export function attachLabel(slug: string, task: string, name: string) {
  return changeLabels(slug, task, (names) =>
    names.includes(name) ? names : [...names, name],
  );
}

export function removeLabel(slug: string, task: string, name: string) {
  return changeLabels(slug, task, (names) => names.filter((n) => n !== name));
}

/** A linked item also adds the edge "this task needs the child", so it can be refused with `dependency_cycle`. */
export async function addChecklistItem(
  slug: string,
  task: string,
  checklist: string,
  item: NewChecklistItem,
): Promise<void> {
  await unwrap(
    api.POST("/api/v1/projects/{slug}/tasks/{id}/checklist-items", {
      params: { path: { slug, id: task } },
      body:
        item.kind === "step"
          ? { checklist, text: item.text }
          : { checklist, linked_task: item.task },
    }),
  );
}

async function patchItem(
  slug: string,
  item: string,
  body: Schemas["ChecklistItemPatchBody"],
): Promise<void> {
  await unwrap(
    api.PATCH("/api/v1/projects/{slug}/checklist-items/{id}", {
      params: { path: { slug, id: item } },
      body,
    }),
  );
}

export function renameChecklistItem(slug: string, item: string, text: string) {
  return patchItem(slug, item, { text });
}

/** Moves the item to `position`, counted from 0 in its checklist. */
export function moveChecklistItem(
  slug: string,
  item: string,
  position: number,
) {
  return patchItem(slug, item, { position });
}

export function tickChecklistItem(slug: string, item: string, done: boolean) {
  return patchItem(slug, item, { done });
}

/** Removing a linked item also removes its edge. */
export function removeChecklistItem(slug: string, item: string) {
  return expectOk(
    api.DELETE("/api/v1/projects/{slug}/checklist-items/{id}", {
      params: { path: { slug, id: item } },
    }),
  );
}

/** Refused with `dependency_cycle` and the cycle in `details.path` when the edge would close a cycle. */
export function addDependency(slug: string, task: string, needs: string) {
  return expectOk(
    api.POST("/api/v1/projects/{slug}/tasks/{id}/dependencies", {
      params: { path: { slug, id: task } },
      body: { needs },
    }),
  );
}

export function removeDependency(slug: string, task: string, needs: string) {
  return expectOk(
    api.DELETE("/api/v1/projects/{slug}/tasks/{id}/dependencies/{needs}", {
      params: { path: { slug, id: task, needs } },
    }),
  );
}

/** The daemon serves no route that sets or clears the mark, so these refuse without a request. The controls that call them mount only when the task read allows the change, which it does not. */
export function setHumanReview(
  slug: string,
  task: string,
  mark: { reviewer: string | null; reason: string },
): Promise<void> {
  return Promise.reject(
    humanReviewUnavailable(slug, task, { reviewer: mark.reviewer }),
  );
}

export function clearHumanReview(slug: string, task: string): Promise<void> {
  return Promise.reject(humanReviewUnavailable(slug, task, {}));
}

function humanReviewUnavailable(
  slug: string,
  task: string,
  more: Record<string, unknown>,
): ApiError {
  return new ApiError("not_found", "the daemon serves no human-review route", {
    project: slug,
    task,
    ...more,
  });
}

/** Refused with `invalid_request` and `details.reason` `proposal_open` while a proposal is open. */
export function archiveTask(slug: string, task: string) {
  return expectOk(
    api.POST("/api/v1/projects/{slug}/tasks/{id}/archive", {
      params: { path: { slug, id: task } },
    }),
  );
}

function idNumber(id: string): number {
  return Number(id.slice(id.indexOf("-") + 1));
}

/** The messages linked to the task, oldest first: every message of the threads the daemon finds for it that names the task. */
export async function getTaskMessages(
  slug: string,
  task: string,
): Promise<TaskMessage[]> {
  const { items } = await unwrap(
    api.GET("/api/v1/projects/{slug}/messages", {
      params: { path: { slug }, query: { task, limit: 200 } },
    }),
  );
  return items
    .flatMap((thread) => thread.messages)
    .filter((message) => message.task === task)
    .sort((a, b) => a.sent_at - b.sent_at || idNumber(a.id) - idNumber(b.id));
}

/** The message is from the signed-in human and linked to the task. */
export async function sendFeedback(
  slug: string,
  task: string,
  feedback: { body: string; to: FeedbackRecipient },
): Promise<void> {
  await unwrap(
    api.POST("/api/v1/projects/{slug}/feedback", {
      params: { path: { slug } },
      body: { task, body: feedback.body, to: feedbackTo(feedback.to) },
    }),
  );
}

/** The event stream keeps the task fresh, so a cached answer never goes stale by time. */
export const taskQuery = (slug: string, task: string) =>
  queryOptions({
    queryKey: keys.task(slug, task),
    queryFn: () => getTask(slug, task),
    staleTime: Infinity,
  });

/** Under the messages prefix, so every message event refreshes it. */
export const taskMessagesQuery = (slug: string, task: string) =>
  queryOptions({
    queryKey: keys.taskMessages(slug, task),
    queryFn: () => getTaskMessages(slug, task),
    staleTime: Infinity,
  });

export const labelsQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.labels(slug),
    queryFn: () => listLabels(slug),
    staleTime: Infinity,
  });

export const messageTargetsQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.messageTargets(slug),
    queryFn: () => getMessageTargets(slug),
  });
