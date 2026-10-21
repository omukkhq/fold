import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { projectCost, taskCost } from "@/test/cost";
import { isApiError } from "./errors";
import { keys } from "./keys";
import {
  getProjectCost,
  getTaskCost,
  projectCostQuery,
  taskCostQuery,
} from "./cost";

afterEach(() => {
  vi.unstubAllGlobals();
});

const BASE = "/api/v1/projects/demo";

describe("cost", () => {
  it("reads the project's cost since a time or of all time", async () => {
    const view = projectCost(0);
    const calls = stubFetch({ [`GET ${BASE}/cost`]: { body: view } });
    expect(await getProjectCost("demo", 1_000)).toEqual(view);
    expect(calls[0]?.search.get("since")).toBe("1000");
    await getProjectCost("demo", null);
    expect(calls[1]?.search.has("since")).toBe(false);
  });

  it("reads a task's cost and rejects an unknown task with its code", async () => {
    stubFetch({
      [`GET ${BASE}/cost/tasks/T-12`]: { body: taskCost() },
      [`GET ${BASE}/cost/tasks/T-9`]: errorReply(404, "task_not_found"),
    });
    expect((await getTaskCost("demo", "T-12")).total_micro_usd).toBe(5_000_000);
    await expect(getTaskCost("demo", "T-9")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "task_not_found"),
    );
  });

  it("keeps every cost read under the cost prefix", () => {
    expect(projectCostQuery("demo", "week", 5).queryKey).toEqual(
      keys.projectCost("demo", "week"),
    );
    expect(taskCostQuery("demo", "T-1").queryKey).toEqual(
      keys.taskCost("demo", "T-1"),
    );
    expect(keys.taskCost("demo", "T-1").slice(0, 2)).toEqual(keys.cost("demo"));
  });
});
