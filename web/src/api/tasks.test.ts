import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { daemon, noContent, ok, refuse } from "@/test/daemon";
import { taskView } from "@/test/payloads";
import { apiErrorCode } from "./errors";
import { keys } from "./keys";
import type { components } from "./schema";
import {
  addChecklistItem,
  addDependency,
  archiveTask,
  attachLabel,
  clearHumanReview,
  setHumanReview,
  createLabel,
  createLabelOn,
  createTask,
  feedbackTo,
  getMessageTargets,
  getTask,
  getTaskMessages,
  labelsQuery,
  listLabels,
  messageTargetsQuery,
  moveChecklistItem,
  removeChecklistItem,
  removeDependency,
  removeLabel,
  renameChecklistItem,
  sendFeedback,
  setDescription,
  setTitle,
  taskMessagesQuery,
  taskQuery,
  tickChecklistItem,
} from "./tasks";

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

type MessageView = components["schemas"]["MessageView"];

const P = "/api/v1/projects/demo";
const ui = { name: "ui", color: "sage" };
const data = { name: "data", color: "gold" };

beforeEach(() => {
  daemon.reset();
});

const bodyOf = (route: string) => daemon.made(route)[0]?.body;

describe("reads", () => {
  it("reads one task", async () => {
    daemon.on(`GET ${P}/tasks/T-2`, ok(taskView("T-2")));
    expect((await getTask("demo", "T-2")).task).toBe("T-2");
  });

  it("throws task_not_found for a task the project does not have", async () => {
    daemon.on(`GET ${P}/tasks/T-9`, refuse(404, "task_not_found"));
    await expect(getTask("demo", "T-9")).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "task_not_found",
    );
  });

  it("lists the labels of the project", async () => {
    daemon.on(`GET ${P}/labels`, ok({ labels: [data, ui] }));
    expect(await listLabels("demo")).toEqual([data, ui]);
  });

  it("derives the message targets from the agents: main agents, child workers and every tag once", async () => {
    daemon.on(
      `GET ${P}/agents`,
      ok({
        agents: [
          {
            name: "frontend",
            tags: ["ui", "integrator"],
            workers: [
              { name: "frontend", parent: null },
              { name: "frontend/ui", parent: "frontend" },
            ],
          },
          { name: "backend", tags: ["ui"], workers: [] },
        ],
      }),
    );
    expect(await getMessageTargets("demo")).toEqual({
      agents: ["frontend", "frontend/ui", "backend"],
      tags: ["integrator", "ui"],
    });
  });

  it("keeps the queries under their keys", async () => {
    expect(taskQuery("demo", "T-2").queryKey).toEqual(keys.task("demo", "T-2"));
    expect(taskQuery("demo", "T-2").staleTime).toBe(Infinity);
    expect(messageTargetsQuery("demo").queryKey).toEqual(
      keys.messageTargets("demo"),
    );
    daemon.on(`GET ${P}/labels`, ok({ labels: [ui] }));
    const client = new QueryClient();
    expect(await client.query(labelsQuery("demo"))).toEqual([ui]);
    expect(client.getQueryData(keys.labels("demo"))).toEqual([ui]);
  });
});

describe("writes to a task", () => {
  it("creates a task and answers its id", async () => {
    daemon.on(`POST ${P}/tasks`, ok(taskView("T-17"), 201));
    expect(await createTask("demo", { title: "Reset", description: "d" })).toBe(
      "T-17",
    );
    expect(bodyOf(`POST ${P}/tasks`)).toEqual({
      title: "Reset",
      description: "d",
    });
  });

  it("changes the title and the description with one patch each", async () => {
    daemon.on(`PATCH ${P}/tasks/T-2`, ok(taskView("T-2")));
    await setTitle("demo", "T-2", "New");
    await setDescription("demo", "T-2", "Text");
    expect(daemon.made(`PATCH ${P}/tasks/T-2`).map((c) => c.body)).toEqual([
      { title: "New" },
      { description: "Text" },
    ]);
  });

  it("refuses a write to a closed task with the daemon's code", async () => {
    daemon.on(
      `PATCH ${P}/tasks/T-2`,
      refuse(409, "task_closed", { reason: "archived" }),
    );
    await expect(setTitle("demo", "T-2", "x")).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "task_closed",
    );
  });

  it("archives a task and shows the open proposal refusal by its reason", async () => {
    daemon.on(`POST ${P}/tasks/T-2/archive`, noContent());
    await archiveTask("demo", "T-2");
    daemon.on(
      `POST ${P}/tasks/T-3/archive`,
      refuse(400, "invalid_request", { reason: "proposal_open" }),
    );
    await expect(archiveTask("demo", "T-3")).rejects.toMatchObject({
      code: "invalid_request",
      details: { reason: "proposal_open" },
    });
  });
});

describe("labels", () => {
  it("creates a label", async () => {
    daemon.on(`POST ${P}/labels`, ok(ui, 201));
    expect(await createLabel("demo", ui)).toEqual(ui);
  });

  it("attaches one label with its own route, never sending the label set", async () => {
    daemon.on(`PUT ${P}/tasks/T-2/labels/:name`, ok(taskView("T-2")));
    await attachLabel("demo", "T-2", "data");
    expect(daemon.made(`PUT ${P}/tasks/T-2/labels/data`)).toHaveLength(1);
    expect(daemon.made(`PATCH ${P}/tasks/T-2`)).toHaveLength(0);
    expect(daemon.made(`GET ${P}/tasks/T-2`)).toHaveLength(0);
  });

  it("removes one label with its own route", async () => {
    daemon.on(`DELETE ${P}/tasks/T-2/labels/:name`, ok(taskView("T-2")));
    await removeLabel("demo", "T-2", "ui");
    expect(daemon.made(`DELETE ${P}/tasks/T-2/labels/ui`)).toHaveLength(1);
  });

  it("sends concurrent label changes as separate single-label writes", async () => {
    daemon.on(`PUT ${P}/tasks/T-2/labels/:name`, ok(taskView("T-2")));
    daemon.on(`DELETE ${P}/tasks/T-2/labels/:name`, ok(taskView("T-2")));
    await Promise.all([
      attachLabel("demo", "T-2", "data"),
      removeLabel("demo", "T-2", "ui"),
    ]);
    expect(daemon.made(`PUT ${P}/tasks/T-2/labels/data`)).toHaveLength(1);
    expect(daemon.made(`DELETE ${P}/tasks/T-2/labels/ui`)).toHaveLength(1);
  });

  it("creates a label and puts it on the task, or leaves the task alone when creation is refused", async () => {
    daemon.on(`POST ${P}/labels`, ok(ui, 201));
    daemon.on(`PUT ${P}/tasks/T-2/labels/:name`, ok(taskView("T-2")));
    await createLabelOn("demo", "T-2", ui);
    expect(daemon.made(`PUT ${P}/tasks/T-2/labels/ui`)).toHaveLength(1);
    daemon.reset();
    daemon.on(`POST ${P}/labels`, refuse(409, "label_exists"));
    await expect(createLabelOn("demo", "T-2", ui)).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "label_exists",
    );
    expect(daemon.made(`PUT ${P}/tasks/T-2/labels/ui`)).toHaveLength(0);
  });
});

describe("checklist items", () => {
  it("adds a plain step or a linked task to a named checklist", async () => {
    daemon.on(`POST ${P}/tasks/T-2/checklist-items`, ok({}, 201));
    await addChecklistItem("demo", "T-2", "Subtasks", {
      kind: "step",
      text: "Copy",
    });
    await addChecklistItem("demo", "T-2", "Subtasks", {
      kind: "linked",
      task: "T-5",
    });
    expect(
      daemon.made(`POST ${P}/tasks/T-2/checklist-items`).map((c) => c.body),
    ).toEqual([
      { checklist: "Subtasks", text: "Copy" },
      { checklist: "Subtasks", linked_task: "T-5" },
    ]);
  });

  it("renames, ticks and moves through the item route, and removes it", async () => {
    daemon.on(`PATCH ${P}/checklist-items/ci-4`, ok({}));
    daemon.on(`DELETE ${P}/checklist-items/ci-4`, noContent());
    await renameChecklistItem("demo", "ci-4", "Text");
    await tickChecklistItem("demo", "ci-4", true);
    await moveChecklistItem("demo", "ci-4", 0);
    await removeChecklistItem("demo", "ci-4");
    expect(
      daemon.made(`PATCH ${P}/checklist-items/ci-4`).map((c) => c.body),
    ).toEqual([{ text: "Text" }, { done: true }, { position: 0 }]);
    expect(daemon.made(`DELETE ${P}/checklist-items/ci-4`)).toHaveLength(1);
  });

  it("refuses a hand tick of a linked item with its reason", async () => {
    daemon.on(
      `PATCH ${P}/checklist-items/ci-5`,
      refuse(400, "invalid_request", { reason: "linked_item" }),
    );
    await expect(tickChecklistItem("demo", "ci-5", true)).rejects.toMatchObject(
      {
        details: { reason: "linked_item" },
      },
    );
  });
});

describe("dependencies", () => {
  it("adds and removes an edge", async () => {
    daemon.on(`POST ${P}/tasks/T-2/dependencies`, noContent());
    daemon.on(`DELETE ${P}/tasks/T-2/dependencies/T-1`, noContent());
    await addDependency("demo", "T-2", "T-1");
    await removeDependency("demo", "T-2", "T-1");
    expect(bodyOf(`POST ${P}/tasks/T-2/dependencies`)).toEqual({
      needs: "T-1",
    });
    expect(daemon.made(`DELETE ${P}/tasks/T-2/dependencies/T-1`)).toHaveLength(
      1,
    );
  });

  it("carries the cycle of a refused edge", async () => {
    daemon.on(
      `POST ${P}/tasks/T-1/dependencies`,
      refuse(409, "dependency_cycle", { path: ["T-1", "T-2", "T-1"] }),
    );
    await expect(addDependency("demo", "T-1", "T-2")).rejects.toMatchObject({
      code: "dependency_cycle",
      details: { path: ["T-1", "T-2", "T-1"] },
    });
  });
});

describe("feedback", () => {
  it("maps each recipient to the daemon's grammar", () => {
    expect(feedbackTo({ kind: "holder" })).toBeUndefined();
    expect(feedbackTo({ kind: "agent", name: "frontend/ui" })).toEqual([
      "frontend/ui",
    ]);
    expect(feedbackTo({ kind: "tag", name: "ui" })).toEqual(["tag:ui"]);
  });

  it("posts the task, the body and the recipients", async () => {
    daemon.on(`POST ${P}/feedback`, ok({}, 201));
    await sendFeedback("demo", "T-2", {
      body: "Hi",
      to: { kind: "tag", name: "ui" },
    });
    await sendFeedback("demo", "T-2", { body: "Hi", to: { kind: "holder" } });
    expect(daemon.made(`POST ${P}/feedback`).map((c) => c.body)).toEqual([
      { task: "T-2", body: "Hi", to: ["tag:ui"] },
      { task: "T-2", body: "Hi" },
    ]);
  });
});

describe("the human-review writes", () => {
  const route = `${P}/tasks/T-2/human-review`;

  it("puts the reviewer and the reason and answers with the mark", async () => {
    const mark = { reviewer: "tanim", reason: "auth", set_by: "human:ana" };
    daemon.on(`PUT ${route}`, ok(mark));
    await expect(
      setHumanReview("demo", "T-2", { reviewer: "tanim", reason: "auth" }),
    ).resolves.toEqual(mark);
    expect(bodyOf(`PUT ${route}`)).toEqual({
      reviewer: "tanim",
      reason: "auth",
    });
  });

  it("deletes the mark and answers with the task", async () => {
    daemon.on(`DELETE ${route}`, ok(taskView("T-2")));
    expect((await clearHumanReview("demo", "T-2")).task).toBe("T-2");
    expect(daemon.made(`DELETE ${route}`)).toHaveLength(1);
  });

  it("carries the refusal code", async () => {
    daemon.on(
      `PUT ${route}`,
      refuse(404, "unknown_human", { reviewer: "nobody" }),
    );
    await expect(
      setHumanReview("demo", "T-2", { reviewer: "nobody", reason: "auth" }),
    ).rejects.toSatisfy((e: unknown) => apiErrorCode(e) === "unknown_human");
  });
});

describe("the task's messages", () => {
  const message = (
    n: number,
    over: Partial<MessageView> = {},
  ): MessageView => ({
    id: `m-${String(n)}`,
    thread: "m-1",
    sender: "frontend/ui",
    recipients: [],
    body: "b",
    proposal: null,
    request: null,
    reply_to: null,
    task: "T-2",
    sent_at: 1_000,
    choices: [],
    header: null,
    ...over,
  });
  const thread = (messages: MessageView[]) => ({
    id: messages[0]?.thread ?? "m-1",
    awaiting_human: false,
    last_at: 0,
    messages,
    participants: [],
    proposal: null,
    task: "T-2",
  });

  it("reads the threads of the task and keeps its messages, oldest first", async () => {
    daemon.on(
      `GET ${P}/messages`,
      ok({
        items: [
          thread([message(10, { sent_at: 3_000, thread: "m-10" })]),
          thread([
            message(1),
            message(2, { task: "T-5" }),
            message(9, { sent_at: 2_000 }),
            message(3, { sent_at: 2_000 }),
          ]),
        ],
        next: null,
      }),
    );
    const query = taskMessagesQuery("demo", "T-2");
    expect(query.queryKey).toEqual(["demo", "messages", "task", "T-2"]);
    expect(query.staleTime).toBe(Infinity);
    const read = await getTaskMessages("demo", "T-2");
    expect(read.items.map((m) => m.id)).toEqual(["m-1", "m-3", "m-9", "m-10"]);
    expect(read.more).toBe(false);
    expect(daemon.calls[0]?.path).toBe(`${P}/messages?task=T-2&limit=200`);
  });

  it("throws the refusal code", async () => {
    daemon.on(`GET ${P}/messages`, refuse(404, "project_not_found"));
    await expect(getTaskMessages("demo", "T-2")).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "project_not_found",
    );
  });
});
