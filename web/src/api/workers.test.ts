import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "@/test/stubFetch";
import {
  needsItem,
  permissionQuestion,
  workerList,
  workerProcess,
} from "@/test/workers";
import { isApiError } from "./errors";
import {
  answerWorkerQuestion,
  listNeedsYou,
  listWorkerQuestions,
  listWorkers,
  restartWorker,
  startWorker,
  stopWorker,
} from "./workers";

afterEach(() => {
  vi.unstubAllGlobals();
});

const BASE = "/api/v1/projects/demo";

describe("workers", () => {
  it("lists, starts and stops workers", async () => {
    const list = workerList({ workers: [workerProcess("web-lead")] });
    const calls = stubFetch({
      [`GET ${BASE}/workers`]: { body: list },
      [`POST ${BASE}/workers`]: {
        status: 201,
        body: workerProcess("web-lead", { state: "starting" }),
      },
      [`POST ${BASE}/workers/web-lead/stop`]: {
        body: workerProcess("web-lead", { state: "stopping" }),
      },
    });
    expect(await listWorkers("demo")).toEqual(list);
    const started = await startWorker("demo", {
      role: "area_lead",
      area: "web",
    });
    expect(started.state).toBe("starting");
    expect(calls[1]?.body).toEqual({ role: "area_lead", area: "web" });
    expect((await stopWorker("demo", "web-lead")).state).toBe("stopping");
  });

  it("starts a worker in a new session with the instructions", async () => {
    const calls = stubFetch({
      [`POST ${BASE}/workers/web-lead/restart`]: {
        body: workerProcess("web-lead", { state: "starting" }),
      },
    });
    const restarted = await restartWorker("demo", "web-lead", {
      instructions: "Plan again.",
    });
    expect(restarted.state).toBe("starting");
    expect(calls[0]?.body).toEqual({ instructions: "Plan again." });
  });

  it("rejects a refused start with its code", async () => {
    stubFetch({ [`POST ${BASE}/workers`]: errorReply(409, "area_has_lead") });
    await expect(
      startWorker("demo", { role: "area_lead", area: "web" }),
    ).rejects.toSatisfy((e: unknown) => isApiError(e, "area_has_lead"));
  });
});

describe("questions", () => {
  it("lists the open questions and sends an answer", async () => {
    const question = permissionQuestion();
    const calls = stubFetch({
      [`GET ${BASE}/worker-questions`]: { body: { items: [question] } },
      [`POST ${BASE}/worker-questions/pq-3/answer`]: {
        body: {
          ...question,
          answer: {
            behavior: "deny",
            answered_by: "human:tanim",
            message: null,
            answered_at: 2,
          },
        },
      },
    });
    expect(await listWorkerQuestions("demo")).toEqual([question]);
    const answered = await answerWorkerQuestion("demo", "pq-3", "deny");
    expect(answered.answer?.behavior).toBe("deny");
    expect(calls[1]?.body).toEqual({ behavior: "deny" });
  });

  it("lists what needs a human", async () => {
    const item = needsItem();
    stubFetch({ [`GET ${BASE}/needs-you`]: { body: { items: [item] } } });
    expect(await listNeedsYou("demo")).toEqual([item]);
  });
});
