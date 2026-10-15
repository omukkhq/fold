import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetTasks } from "../../fixtures/tasks";
import { ApiError } from "./errors";
import { keys } from "./keys";
import {
  addChecklistItem,
  createTask,
  addDependency,
  addLabel,
  clearHumanReview,
  getMessageTargets,
  getTask,
  messageTargetsQuery,
  moveChecklistItem,
  removeChecklistItem,
  removeDependency,
  removeLabel,
  renameChecklistItem,
  sendFeedback,
  setDescription,
  setHumanReview,
  setTitle,
  taskQuery,
  tickChecklistItem,
  type TaskDetail,
} from "./tasks";

beforeEach(() => {
  vi.useFakeTimers();
  resetTasks();
});
afterEach(() => {
  vi.useRealTimers();
});

async function settle<T>(promise: Promise<T>): Promise<T> {
  const settled = promise.then(
    (value) => ({ ok: true as const, value }),
    (error: unknown) => ({ ok: false as const, error }),
  );
  await vi.runAllTimersAsync();
  const result = await settled;
  if (!result.ok) {
    throw result.error;
  }
  return result.value;
}

async function refused(promise: Promise<unknown>): Promise<ApiError> {
  try {
    await settle(promise);
  } catch (error) {
    if (error instanceof ApiError) {
      return error;
    }
    throw error;
  }
  throw new Error("the write was not refused");
}

function read(task = "T-2"): Promise<TaskDetail> {
  return settle(getTask("demo", task));
}

describe("the task fixture", () => {
  it("gives T-2 every section of W03", async () => {
    const task = await read();
    expect(task.title).toBe("Build the login form with validation");
    expect(task.status).toBe("in_progress");
    expect(task.labels.length).toBeGreaterThan(0);
    expect(task.checklists[0]?.name).toBe("Subtasks");
    const kinds = task.checklists[0]?.items.map((i) => i.kind);
    expect(kinds).toContain("step");
    expect(kinds).toContain("linked");
    expect(task.dependencies.map((d) => d.direction).sort()).toEqual([
      "needed_by",
      "needs",
    ]);
    expect(task.claims[0]?.kind).toBe("implementation");
    expect(task.workspace?.live?.files.length).toBe(4);
    expect(task.checkpoints.length).toBeGreaterThan(1);
    expect(task.changesets.length).toBeGreaterThan(0);
    expect(task.messages.length).toBe(2);
    expect(task.history.length).toBeGreaterThan(2);
    expect(task.description).toContain("| Code | Message |");
    expect(task.description).toContain("- [x]");
  });

  it("opens every card on the board, with the proposal, integration and human review it shows", async () => {
    const done = await read("T-13");
    expect(done.proposal?.state).toBe("integrated");
    expect(done.integration?.revision).toBe("r8");
    const marked = await read("T-3");
    expect(marked.human_review?.reviewer).toBe("tanim");
    const blocked = await read("T-10");
    expect(blocked.blocked).toBe(true);
  });

  it("refuses an unknown task and every task of another project with task_not_found", async () => {
    expect((await refused(getTask("demo", "T-999"))).code).toBe(
      "task_not_found",
    );
    expect((await refused(getTask("ledger", "T-2"))).code).toBe(
      "task_not_found",
    );
  });

  it("lists agents and tags for the feedback box, and none for an empty project", async () => {
    expect((await settle(getMessageTargets("demo"))).agents).toContain(
      "frontend",
    );
    expect(await settle(getMessageTargets("ledger"))).toEqual({
      agents: [],
      tags: [],
    });
  });

  it("keys the queries by slug and public id", () => {
    expect(taskQuery("demo", "T-2").queryKey).toEqual(keys.task("demo", "T-2"));
    expect(messageTargetsQuery("demo").queryKey).toEqual(
      keys.messageTargets("demo"),
    );
  });
});

describe("creating a task", () => {
  it("numbers it after the project's tasks and calculates Todo", async () => {
    const id = await settle(
      createTask("demo", { title: "Password reset", description: "" }),
    );
    expect(id).toBe("T-17");
    const task = await read(id);
    expect(task).toMatchObject({
      title: "Password reset",
      status: "todo",
      dependencies: [],
    });
    expect(task.history[0]?.actor).toBe("human:tanim");
  });

  it("starts an empty project at T-1 and puts the task on its board", async () => {
    expect(
      await settle(createTask("ledger", { title: "First", description: "x" })),
    ).toBe("T-1");
    const { board } = await import("../../fixtures/board");
    const cards = board("ledger").columns.flatMap((c) => c.cards);
    expect(cards.map((c) => [c.task, c.title, c.status])).toEqual([
      ["T-1", "First", "todo"],
    ]);
  });
});

describe("the writes", () => {
  it("change the title and the description and record the edit", async () => {
    await settle(setTitle("demo", "T-2", "Sign-in form"));
    await settle(setDescription("demo", "T-2", "New text"));
    const task = await read();
    expect(task.title).toBe("Sign-in form");
    expect(task.description).toBe("New text");
    expect(task.history[0]).toMatchObject({
      type: "task.updated",
      actor: "human:tanim",
    });
  });

  it("add a label with the project's colour for a known name, and remove it", async () => {
    await settle(addLabel("demo", "T-9", { name: "ui", color: "plum" }));
    await settle(addLabel("demo", "T-9", { name: "fresh", color: "gold" }));
    await settle(addLabel("demo", "T-9", { name: "fresh", color: "teal" }));
    let labels = (await read("T-9")).labels;
    expect(labels).toContainEqual({ name: "ui", color: "sage" });
    expect(labels.filter((l) => l.name === "fresh")).toEqual([
      { name: "fresh", color: "gold" },
    ]);
    await settle(removeLabel("demo", "T-9", "fresh"));
    labels = (await read("T-9")).labels;
    expect(labels.map((l) => l.name)).not.toContain("fresh");
  });

  it("add, rename, tick, move and remove a plain step", async () => {
    await settle(
      addChecklistItem("demo", "T-2", "Subtasks", {
        kind: "step",
        text: "Write the copy",
      }),
    );
    let items = (await read()).checklists[0]?.items ?? [];
    const added = items.at(-1);
    expect(added).toMatchObject({ kind: "step", text: "Write the copy" });
    const id = added?.id ?? "";
    await settle(
      renameChecklistItem("demo", "T-2", id, "Write the error copy"),
    );
    await settle(tickChecklistItem("demo", "T-2", id, true));
    await settle(moveChecklistItem("demo", "T-2", id, 0));
    items = (await read()).checklists[0]?.items ?? [];
    expect(items[0]).toMatchObject({
      id,
      text: "Write the error copy",
      done: true,
    });
    await settle(removeChecklistItem("demo", "T-2", id));
    items = (await read()).checklists[0]?.items ?? [];
    expect(items.map((i) => i.id)).not.toContain(id);
  });

  it("start a new checklist for a name the task does not have", async () => {
    await settle(
      addChecklistItem("demo", "T-13", "Follow-ups", {
        kind: "step",
        text: "Docs",
      }),
    );
    const lists = (await read("T-13")).checklists;
    expect(lists.map((c) => c.name)).toEqual(["Follow-ups"]);
  });

  it("add the edge with a linked item and remove the edge with it", async () => {
    await settle(
      addChecklistItem("demo", "T-9", "Subtasks", {
        kind: "linked",
        task: "T-14",
      }),
    );
    let task = await read("T-9");
    const linked = task.checklists[0]?.items.at(-1);
    expect(linked).toMatchObject({ kind: "linked", task: "T-14", done: true });
    expect(task.dependencies).toContainEqual(
      expect.objectContaining({
        task: "T-14",
        direction: "needs",
        origin: "checklist",
      }),
    );
    await settle(removeChecklistItem("demo", "T-9", linked?.id ?? ""));
    task = await read("T-9");
    expect(task.dependencies.map((d) => d.task)).not.toContain("T-14");
  });

  it("refuse a hand tick or a rename on a linked item", async () => {
    const tick = await refused(tickChecklistItem("demo", "T-2", "T-2-5", true));
    expect(tick.code).toBe("invalid_request");
    expect(tick.details.reason).toBe("linked_item");
    const rename = await refused(
      renameChecklistItem("demo", "T-2", "T-2-5", "x"),
    );
    expect(rename.details.reason).toBe("linked_item");
    expect(
      (await refused(tickChecklistItem("demo", "T-2", "nope", true))).code,
    ).toBe("not_found");
  });

  it("add and remove a direct dependency", async () => {
    await settle(addDependency("demo", "T-9", "T-16"));
    expect((await read("T-9")).dependencies).toContainEqual(
      expect.objectContaining({ task: "T-16", origin: "direct" }),
    );
    expect((await read("T-16")).dependencies).toContainEqual(
      expect.objectContaining({ task: "T-9", direction: "needed_by" }),
    );
    await settle(removeDependency("demo", "T-9", "T-16"));
    expect((await read("T-9")).dependencies).toEqual([]);
  });

  it("refuse a cycle with its path and leave the edges as they were", async () => {
    const before = (await read()).dependencies;
    const error = await refused(addDependency("demo", "T-2", "T-5"));
    expect(error.code).toBe("dependency_cycle");
    expect(error.details.cycle).toEqual(["T-2", "T-5", "T-2"]);
    expect((await read()).dependencies).toEqual(before);
  });

  it("refuse a self edge, an unknown task, a duplicate, a Done task and a linked item's edge", async () => {
    expect(
      (await refused(addDependency("demo", "T-2", "T-2"))).details.reason,
    ).toBe("self_dependency");
    expect((await refused(addDependency("demo", "T-2", "T-99"))).code).toBe(
      "task_not_found",
    );
    expect((await refused(addDependency("demo", "T-5", "T-2"))).code).toBe(
      "dependency_exists",
    );
    expect((await refused(addDependency("demo", "T-13", "T-9"))).code).toBe(
      "task_closed",
    );
    expect(
      (await refused(removeDependency("demo", "T-2", "T-13"))).details.reason,
    ).toBe("checklist_origin");
    expect((await refused(removeDependency("demo", "T-2", "T-9"))).code).toBe(
      "dependency_not_found",
    );
  });

  it("turn human review on with a reviewer and a reason, and off", async () => {
    await settle(
      setHumanReview("demo", "T-2", { reviewer: "tanim", reason: "Auth work" }),
    );
    expect((await read()).human_review).toMatchObject({
      reviewer: "tanim",
      reason: "Auth work",
      set_by: "human:tanim",
    });
    await settle(clearHumanReview("demo", "T-2"));
    expect((await read()).human_review).toBeNull();
  });

  it("send feedback to the claim holder, an agent or a tag, from the signed-in human", async () => {
    await settle(
      sendFeedback("demo", "T-2", { body: "One", to: { kind: "holder" } }),
    );
    await settle(
      sendFeedback("demo", "T-2", {
        body: "Two",
        to: { kind: "agent", name: "review" },
      }),
    );
    await settle(
      sendFeedback("demo", "T-2", {
        body: "Three",
        to: { kind: "tag", name: "frontend" },
      }),
    );
    const sent = (await read()).messages.slice(-3);
    expect(sent.map((m) => [m.from, m.to, m.body])).toEqual([
      ["human:tanim", "frontend/ui", "One"],
      ["human:tanim", "review", "Two"],
      ["human:tanim", "tag:frontend", "Three"],
    ]);
    const holderless = await refused(
      sendFeedback("demo", "T-9", { body: "x", to: { kind: "holder" } }),
    );
    expect(holderless.code).toBe("invalid_request");
  });

  it("refuse every write to a task the project does not have", async () => {
    expect((await refused(setTitle("demo", "T-99", "x"))).code).toBe(
      "task_not_found",
    );
  });
});
