import { queryOptions } from "@tanstack/react-query";
import { api } from "./client";
import { keys } from "./keys";
import { unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type ProjectCost = Schemas["ProjectCostView"];
export type TaskCost = Schemas["TaskCostView"];
export type TaskWorkerCost = Schemas["TaskWorkerCost"];
export type CostSlot = Schemas["CostSlot"];
export type WorkerCost = Schemas["WorkerCost"];
export type TokenCounts = Schemas["TokenCounts"];

/** What the project's supervised workers cost since `since` (UTC milliseconds), or in all with `null`. */
export function getProjectCost(
  slug: string,
  since: number | null,
): Promise<ProjectCost> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/cost", {
      params: {
        path: { slug },
        query: since === null ? {} : { since },
      },
    }),
  );
}

/** What one task cost, per worker that built it or reviewed one of its proposals. Rejects with `task_not_found`. */
export function getTaskCost(slug: string, task: string): Promise<TaskCost> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/cost/tasks/{id}", {
      params: { path: { slug, id: task } },
    }),
  );
}

/** The cost of one range of the cost view; `since` is the range's start, which the caller reads from the viewer's clock. The end of a worker's turn refreshes it. */
export const projectCostQuery = (
  slug: string,
  range: string,
  since: number | null,
) =>
  queryOptions({
    queryKey: keys.projectCost(slug, range, since),
    queryFn: () => getProjectCost(slug, since),
    staleTime: Infinity,
  });

export const taskCostQuery = (slug: string, task: string) =>
  queryOptions({
    queryKey: keys.taskCost(slug, task),
    queryFn: () => getTaskCost(slug, task),
    staleTime: Infinity,
  });
