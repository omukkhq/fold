import { queryOptions } from "@tanstack/react-query";
import type { BoardLabel } from "./board";
import { api } from "./client";
import { keys } from "./keys";
import { readPages, type Pages } from "./pages";
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

export type TaskWorkspaceInfo = Schemas["TaskWorkspaceInfo"];
export type TaskChangeset = Schemas["TaskChangeset"];
export type ReviewVerdict = Schemas["ReviewVerdict"];
export type TaskReview = Schemas["TaskReview"];
export type TaskProposal = Schemas["TaskProposal"];
export type TaskIntegration = Schemas["TaskIntegration"];
export type TaskEvent = Schemas["TaskEvent"];
export type TaskKind = Schemas["TaskKind"];
export type TaskReport = Schemas["TaskReport"];
export type Blocker = Schemas["BlockerMark"];

export type TaskDetail = Schemas["TaskView"];

export type TaskReader = Schemas["TaskReader"];
export type TaskReaders = Schemas["TaskReaders"];
export type TaskThread = Schemas["ThreadView"];

export type FeedbackRecipient =
  | { kind: "holder" }
  | { kind: "agent"; name: string };

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
  }
}

export function getTask(slug: string, task: string): Promise<TaskDetail> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/tasks/{id}", {
      params: { path: { slug, id: task } },
    }),
  );
}

/** Who the task's feedback box offers as readers, the default first. */
export function getTaskReaders(
  slug: string,
  task: string,
): Promise<TaskReaders> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/tasks/{id}/readers", {
      params: { path: { slug, id: task } },
    }),
  );
}

/** Answers the new task's public id. The kind is sent only when it is not `proposal`, the daemon's default. */
export async function createTask(
  slug: string,
  input: {
    title: string;
    description: string;
    kind?: TaskKind;
    labels?: readonly string[];
  },
): Promise<string> {
  const { kind, labels, ...fields } = input;
  const body: Schemas["CreateTaskBody"] = {
    ...fields,
    ...(kind === undefined || kind === "proposal" ? {} : { kind }),
    ...(labels === undefined || labels.length === 0
      ? {}
      : { labels: [...labels] }),
  };
  const created = await unwrap(
    api.POST("/api/v1/projects/{slug}/tasks", {
      params: { path: { slug } },
      body,
    }),
  );
  return created.task;
}

/** The lead check of a reported `no_proposal` task, refused with `task_not_reported` when no report waits; for a `human` task, the human marks the person's work Done. */
export async function completeTask(slug: string, task: string): Promise<void> {
  await unwrap(
    api.POST("/api/v1/projects/{slug}/tasks/{id}/complete", {
      params: { path: { slug, id: task } },
    }),
  );
}

/** Returns a reported `no_proposal` task to its task worker with `note`, which the daemon gives the resumed task worker. */
export async function sendBackTask(
  slug: string,
  task: string,
  note: string,
): Promise<void> {
  await unwrap(
    api.POST("/api/v1/projects/{slug}/tasks/{id}/send-back", {
      params: { path: { slug, id: task } },
      body: { note },
    }),
  );
}

/** Puts a blocker on the task with `reason`, or changes the reason: no agent takes the task until a human clears it. */
export async function setBlocker(
  slug: string,
  task: string,
  reason: string,
): Promise<void> {
  await unwrap(
    api.PUT("/api/v1/projects/{slug}/tasks/{id}/blocker", {
      params: { path: { slug, id: task } },
      body: { reason },
    }),
  );
}

export async function clearBlocker(slug: string, task: string): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/projects/{slug}/tasks/{id}/blocker", {
      params: { path: { slug, id: task } },
    }),
  );
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

/** Replaces the task's whole label set in one write, so an area changes without a moment with two areas or none. */
export function setLabels(slug: string, task: string, labels: string[]) {
  return update(slug, task, { labels });
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

/** Puts one label on the task; the daemon keeps the labels the task already has, so concurrent changes never undo each other. */
export async function attachLabel(
  slug: string,
  task: string,
  name: string,
): Promise<void> {
  await unwrap(
    api.PUT("/api/v1/projects/{slug}/tasks/{id}/labels/{name}", {
      params: { path: { slug, id: task, name } },
    }),
  );
}

export async function removeLabel(
  slug: string,
  task: string,
  name: string,
): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/projects/{slug}/tasks/{id}/labels/{name}", {
      params: { path: { slug, id: task, name } },
    }),
  );
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

/** A human may change an existing mark; a named reviewer the daemon does not know is refused with `unknown_human`. */
export function setHumanReview(
  slug: string,
  task: string,
  mark: { reviewer: string | null; reason: string },
): Promise<Schemas["HumanReviewMark"]> {
  return unwrap(
    api.PUT("/api/v1/projects/{slug}/tasks/{id}/human-review", {
      params: { path: { slug, id: task } },
      body: { reviewer: mark.reviewer, reason: mark.reason },
    }),
  );
}

/** Answers with the task as it stands without the mark; clearing a task that has none changes nothing. */
export function clearHumanReview(
  slug: string,
  task: string,
): Promise<Schemas["TaskView"]> {
  return unwrap(
    api.DELETE("/api/v1/projects/{slug}/tasks/{id}/human-review", {
      params: { path: { slug, id: task } },
    }),
  );
}

/** Refused with `invalid_request` and `details.reason` `proposal_open` while a proposal is open. */
export function archiveTask(slug: string, task: string) {
  return expectOk(
    api.POST("/api/v1/projects/{slug}/tasks/{id}/archive", {
      params: { path: { slug, id: task } },
    }),
  );
}

const MESSAGE_PAGE_SIZE = 200;
const MESSAGE_PAGES = 5;

/** The threads linked to the task that the signed-in human takes part in, newest first, each with every message, from at most `MESSAGE_PAGES` pages; `more` says that older threads were left out. The daemon filters them with `mine`; threads only between agents stay in Activity. */
export async function getTaskMessages(
  slug: string,
  task: string,
): Promise<Pages<TaskThread>> {
  return readPages(async (before) => {
    const page = await unwrap(
      api.GET("/api/v1/projects/{slug}/messages", {
        params: {
          path: { slug },
          query: { task, mine: true, before, limit: MESSAGE_PAGE_SIZE },
        },
      }),
    );
    return { items: page.items, next: page.next };
  }, MESSAGE_PAGES);
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

/** Under the agents prefix, so a worker or agent event refreshes the area leads and the main agent; a task or claim event of the task refreshes it too. */
export const taskReadersQuery = (slug: string, task: string) =>
  queryOptions({
    queryKey: keys.taskReaders(slug, task),
    queryFn: () => getTaskReaders(slug, task),
    staleTime: Infinity,
  });
