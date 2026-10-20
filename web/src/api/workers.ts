import { api } from "./client";
import { unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type WorkerProcess = Schemas["WorkerProcessEntry"];
export type WorkerRole = Schemas["WorkerRole"];
export type WorkerState = Schemas["WorkerProcessState"];
export type AreaEntry = Schemas["AreaEntry"];
export type WorkerList = Schemas["WorkerList"];
export type RoleEntry = Schemas["RoleEntry"];
export type StartWorkerBody = Schemas["StartWorkerBody"];
export type RestartWorkerBody = Schemas["RestartWorkerBody"];
export type PreviousSession = Schemas["PreviousSessionEntry"];
export type WorkerQuestion = Schemas["WorkerQuestionEntry"];
export type QuestionAnswer = Schemas["QuestionAnswer"];
export type Behavior = Schemas["Behavior"];
export type NeedsYouItem = Schemas["NeedsYouEntry"];
export type NeedsYouKind = Schemas["NeedsYouItemKind"];
export type WorkerOutput = Schemas["WorkerOutput"];

/** The areas in name order and every worker fold runs or ran, in start order. */
export function listWorkers(slug: string): Promise<WorkerList> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/workers", {
      params: { path: { slug } },
    }),
  );
}

/** A named human starts an area lead or the review lead. Rejects with the daemon's codes, such as `area_has_lead` or `worker_limit_reached`. */
export function startWorker(
  slug: string,
  body: StartWorkerBody,
): Promise<WorkerProcess> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/workers", {
      params: { path: { slug } },
      body,
    }),
  );
}

/** A named human stops a worker; it answers with the worker after the stop. Rejects with `worker_process_not_found` or `human_name_required`. */
export function stopWorker(slug: string, name: string): Promise<WorkerProcess> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/workers/{name}/stop", {
      params: { path: { slug, name } },
    }),
  );
}

/** A named human starts a worker again in a new Claude Code session; it answers with the worker in its new session. Rejects with the daemon's codes, such as `project_paused` or `worker_limit_reached`. */
export function restartWorker(
  slug: string,
  name: string,
  body: RestartWorkerBody,
): Promise<WorkerProcess> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/workers/{name}/restart", {
      params: { path: { slug, name } },
      body,
    }),
  );
}

/** The open permission questions, oldest first. */
export async function listWorkerQuestions(
  slug: string,
): Promise<WorkerQuestion[]> {
  const { items } = await unwrap(
    api.GET("/api/v1/projects/{slug}/worker-questions", {
      params: { path: { slug } },
    }),
  );
  return items;
}

/** A named human allows or denies a tool call; it answers with the question and its answer. Rejects with `permission_question_closed` when the question already has its answer or its worker stopped. */
export function answerWorkerQuestion(
  slug: string,
  id: string,
  behavior: Behavior,
): Promise<WorkerQuestion> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/worker-questions/{id}/answer", {
      params: { path: { slug, id } },
      body: { behavior },
    }),
  );
}

/** Every item that waits for a human, oldest first. */
export async function listNeedsYou(slug: string): Promise<NeedsYouItem[]> {
  const { items } = await unwrap(
    api.GET("/api/v1/projects/{slug}/needs-you", {
      params: { path: { slug } },
    }),
  );
  return items;
}
