import { describe, expect, it } from "vitest";
import type { BoardCard, TaskStatus } from "@/api/board";
import { agent, worker, workspace } from "@/test/agents";
import { boardCard } from "@/test/payloads";
import {
  areaEntry,
  batch,
  member,
  permissionQuestion,
  workerList,
  workerProcess,
} from "@/test/workers";
import {
  actorAgent,
  areaLabel,
  areaNote,
  areaProfile,
  askingRow,
  batchedProposals,
  contextText,
  costText,
  hasNoLeads,
  heldBatch,
  isPausedWorker,
  isEmptyTeam,
  mainAgentName,
  mainPill,
  ownedTasks,
  pillSpec,
  rowError,
  roleModel,
  roleName,
  stepOf,
  stopControl,
  teamGroups,
  workerPill,
  waitsForTaskWorkers,
  workerStep,
  workspaceOf,
  type TeamInput,
} from "./team";

function card(
  task: string,
  label: string,
  status: TaskStatus = "todo",
): BoardCard {
  return boardCard(task, { status, labels: [{ name: label, color: "slate" }] });
}

function input(fields: Partial<TeamInput> = {}): TeamInput {
  return {
    workers: workerList(),
    agents: [],
    questions: [],
    steps: {},
    batches: [],
    cards: [],
    labels: [],
    human: null,
    ...fields,
  };
}

describe("numbers", () => {
  it("words cost to the cent and context in thousands", () => {
    expect(costText(5_870_000)).toBe("$5.87");
    expect(costText(0)).toBe("$0.00");
    expect(contextText(164_000)).toBe("164k");
    expect(contextText(640)).toBe("640");
    expect(contextText(0)).toBe("-");
  });
});

describe("pills", () => {
  it("gives each state its tone and the main agent its presence", () => {
    expect(workerPill("stopping")).toEqual({
      label: "stopping",
      tone: "stopping",
    });
    expect(mainPill("active")).toEqual({ label: "active", tone: "working" });
    expect(mainPill("idle")).toEqual({ label: "idle", tone: "idle" });
    expect(mainPill("offline")).toEqual({ label: "offline", tone: "stopped" });
    expect(pillSpec("failed").fill).toBe("bg-danger-soft");
    expect(pillSpec("ready").text).toBe("text-success");
  });

  it("names every role", () => {
    expect(roleName("area_lead")).toBe("Area lead");
    expect(roleName("review_lead")).toBe("Review lead");
    expect(roleName("task_worker")).toBe("Task worker");
    expect(roleName("reviewer")).toBe("Reviewer");
  });

  it("takes the agent of a worker actor", () => {
    expect(actorAgent("web-lead")).toBe("web-lead");
    expect(actorAgent("web-lead/sub")).toBe("web-lead");
    expect(actorAgent("human:tanim")).toBe("human:tanim");
  });

  it("counts one owned task in the singular", () => {
    expect(
      areaNote({
        key: "area:web",
        kind: "area",
        label: { name: "web", color: "slate" },
        rows: [],
        owned: 1,
        area: "web",
        profile: { facts: [], instructions: null },
      }),
    ).toBe("area · lead owns 1 task");
  });

  it("shows only the profile fields an area sets", () => {
    expect(areaProfile(null)).toEqual({ facts: [], instructions: null });
    expect(areaProfile(areaEntry("web", "frontend"))).toEqual({
      facts: [],
      instructions: null,
    });
    expect(
      areaProfile(
        areaEntry("design", "design", {
          instructions: "Draw screens in Pencil.",
          model: "opus",
          mcp: ["fold", "pencil"],
        }),
      ),
    ).toEqual({
      facts: ["model opus", "MCP fold, pencil"],
      instructions: "Draw screens in Pencil.",
    });
    expect(
      areaProfile(areaEntry("api", "api", { mcp: ["fold"] })).facts,
    ).toEqual(["MCP fold"]);
  });

  it("colours an area by its label, in stone for a label the board does not know", () => {
    const areas = [areaEntry("web", "frontend"), areaEntry("api", "server")];
    const labels = [{ name: "frontend", color: "slate" as const }];
    expect(areaLabel(areas, labels, "web")).toEqual({
      name: "frontend",
      color: "slate",
    });
    expect(areaLabel(areas, labels, "api")).toEqual({
      name: "server",
      color: "stone",
    });
    expect(areaLabel(areas, labels, "docs")).toBeNull();
  });
});

describe("steps", () => {
  it("shows a tool call by its tool and other steps by their kind", () => {
    expect(stepOf({ step: "tool_use", tool: "Edit" })).toEqual({
      tool: "Edit",
      text: "",
      faint: false,
    });
    expect(stepOf({ step: "tool_use", tool: null }).text).toBe("calls a tool");
    expect(stepOf({ step: "permission_request", tool: "Bash" })).toEqual({
      tool: "Bash",
      text: "waits for your answer",
      faint: false,
    });
    expect(stepOf({ step: "thinking", tool: null }).text).toBe("thinking");
    expect(stepOf({ step: "future_kind", tool: null }).text).toBe(
      "future_kind",
    );
  });

  it("uses the live step only while the process works", () => {
    const steps = { w: { step: "text", tool: null } };
    expect(workerStep(workerProcess("w"), steps).text).toBe("writing");
    expect(workerStep(workerProcess("w"), {})).toEqual({
      tool: null,
      text: "in a turn",
      faint: true,
    });
    const states = [
      ["starting", "starting the process"],
      ["stopping", "waiting for the process to exit"],
      ["idle", "waiting for its next turn"],
      ["stopped", "stopped"],
    ] as const;
    for (const [state, text] of states) {
      expect(workerStep(workerProcess("w", { state }), steps).text).toBe(text);
    }
    expect(
      workerStep(workerProcess("w", { state: "failed", exit_code: 101 }), {})
        .text,
    ).toBe("exit code 101");
  });

  it("says an idle lead waits for task workers only while its task workers start or work", () => {
    const lead = workerProcess("web-lead", { state: "idle" });
    const taskWorker = (state: "working" | "idle", by = "web-lead") =>
      workerProcess("c", { role: "task_worker", started_by: by, state });
    expect(workerStep(lead, {}, true).text).toBe("waiting for task workers");
    expect(workerStep(lead, {}).text).toBe("waiting for its next turn");
    expect(waitsForTaskWorkers(lead, [lead, taskWorker("working")])).toBe(true);
    expect(waitsForTaskWorkers(lead, [lead, taskWorker("idle")])).toBe(false);
    expect(
      waitsForTaskWorkers(lead, [lead, taskWorker("working", "other")]),
    ).toBe(false);
    const other = workerProcess("o", { started_by: "web-lead" });
    expect(waitsForTaskWorkers(lead, [lead, other])).toBe(false);
    const taskWorkerRow = taskWorker("working");
    expect(waitsForTaskWorkers(taskWorkerRow, [taskWorkerRow])).toBe(false);
  });

  it("reads a role's model from the project config", () => {
    const roles = [
      { role: "area_lead", cache_ttl: "1h", model: "opus" },
      { role: "task_worker", cache_ttl: "5m", model: null },
    ] as const;
    expect(roleModel(roles, "area_lead")).toBe("opus");
    expect(roleModel(roles, "task_worker")).toBeNull();
    expect(roleModel(roles, "review_lead")).toBeNull();
    const [group] = teamGroups(
      input({
        workers: workerList({
          roles: [...roles],
          workers: [workerProcess("web-lead", { model: null })],
        }),
      }),
    );
    expect(group?.rows[0]?.model).toBe("opus");
  });

  it("shows a paused worker as paused, with no Stop, until the resume", () => {
    expect(isPausedWorker("working", true)).toBe(true);
    expect(isPausedWorker("idle", true)).toBe(true);
    expect(isPausedWorker("starting", true)).toBe(true);
    expect(isPausedWorker("stopping", true)).toBe(false);
    expect(isPausedWorker("working", false)).toBe(false);
    const workers = [
      workerProcess("web-lead", { state: "working" }),
      workerProcess("web-worker", { state: "stopped", role: "task_worker" }),
    ];
    const [group] = teamGroups(
      input({
        workers: workerList({
          workers,
          pause: { paused_since: 1_760_000_000_000, paused_by: "human:ana" },
        }),
      }),
    );
    expect(group?.rows[0]).toMatchObject({
      pill: { label: "paused", tone: "paused" },
      step: { text: "paused · resumes in its own session" },
      stop: null,
    });
    expect(pillSpec("paused").fill).toBe("bg-warning-soft");
    const [running] = teamGroups(input({ workers: workerList({ workers }) }));
    expect(running?.rows[0]?.pill).toEqual(workerPill("working"));
  });

  it("shows a worker's last error under its step, and what to check once it cannot reach the model", () => {
    expect(rowError(workerProcess("web-lead"))).toBeNull();
    expect(
      rowError(workerProcess("web-lead", { last_error: "Reached max turns" })),
    ).toEqual({ title: "last error", text: "Reached max turns", hint: null });
    const failing = workerProcess("web-lead", {
      state: "idle",
      last_error: "Invalid key",
      failed_turns: 3,
      model_unreachable: true,
      hint: "check the sign-in, or run `fld doctor`",
    });
    expect(rowError(failing)).toEqual({
      title: "cannot reach the model",
      text: "Invalid key",
      hint: "check the sign-in, or run `fld doctor`",
    });
    const [group] = teamGroups(
      input({ workers: workerList({ workers: [failing] }) }),
    );
    expect(group?.rows[0]?.error?.title).toBe("cannot reach the model");
  });

  it("offers Stop to a live process and a pending stop while it stops", () => {
    expect(stopControl("working")).toBe("stop");
    expect(stopControl("idle")).toBe("stop");
    expect(stopControl("starting")).toBe("stop");
    expect(stopControl("stopping")).toBe("stopping");
    expect(stopControl("stopped")).toBeNull();
    expect(stopControl("failed")).toBeNull();
  });
});

describe("lookups", () => {
  it("finds a worker's workspace on a task", () => {
    const agents = [
      agent("web-worker", {
        workers: [worker("web-worker", { workspaces: [workspace()] })],
      }),
    ];
    expect(workspaceOf(agents, "web-worker", "T-12")).toBe("ws-7");
    expect(workspaceOf(agents, "web-worker", "T-99")).toBeNull();
    expect(workspaceOf(agents, "other", "T-12")).toBeNull();
  });

  it("picks the open batch of a holder, else its newest", () => {
    const old = batch({ id: "rb-1", state: "submitted" });
    const open = batch({ id: "rb-2" });
    expect(heldBatch([old, open], "review-b3")?.id).toBe("rb-2");
    expect(heldBatch([old], "review-b3")?.id).toBe("rb-1");
    expect(heldBatch([old], "nobody")).toBeNull();
  });

  it("counts the proposals of open batches once and the open tasks of a label", () => {
    const batches = [
      batch({ members: [member("P-3"), member("P-4", { in_batch: false })] }),
      batch({ id: "rb-4", members: [member("P-3")] }),
      batch({ id: "rb-1", state: "closed", members: [member("P-9")] }),
    ];
    expect(batchedProposals(batches)).toEqual(["P-3"]);
    const cards = [
      card("T-1", "frontend"),
      card("T-2", "frontend", "done"),
      card("T-3", "backend"),
    ];
    expect(ownedTasks(cards, "frontend")).toBe(1);
  });
});

describe("teamGroups", () => {
  const areas = [
    areaEntry("web", "frontend", { lead: "web-lead" }),
    areaEntry("api", "api", { model: "sonnet" }),
  ];

  it("puts the main agent first, each area with its lead and task workers, then review", () => {
    const groups = teamGroups(
      input({
        human: "tanim",
        agents: [
          agent("main"),
          agent("web-worker", {
            workers: [worker("web-worker", { workspaces: [workspace()] })],
          }),
        ],
        labels: [{ name: "frontend", color: "slate" }],
        cards: [card("T-12", "frontend"), card("T-13", "frontend")],
        questions: [permissionQuestion({ worker: "web-worker" })],
        steps: { "web-worker": { step: "tool_use", tool: "Edit" } },
        batches: [
          batch({
            holder: "review-b3",
            members: [member("P-3"), member("P-4")],
          }),
        ],
        workers: workerList({
          areas,
          workers: [
            workerProcess("web-lead"),
            workerProcess("web-worker", {
              role: "task_worker",
              started_by: "web-lead",
              tasks: ["T-12", "T-13"],
              model: null,
            }),
            workerProcess("review", { role: "review_lead", area: null }),
            workerProcess("review-b3", {
              role: "reviewer",
              area: null,
              started_by: "review",
            }),
          ],
        }),
      }),
    );
    expect(groups.map((g) => g.key)).toEqual([
      "main",
      "area:web",
      "area:api",
      "review",
    ]);
    const [main, web, api, review] = groups;
    expect(main?.rows[0]).toMatchObject({
      name: "main",
      roleLine: "Main agent · your terminal session",
      bound: { kind: "person", name: "human:tanim" },
      stop: null,
      model: "-",
    });
    expect(web?.label).toEqual({ name: "frontend", color: "slate" });
    expect(web?.owned).toBe(2);
    expect(web?.rows.map((r) => [r.name, r.child])).toEqual([
      ["web-lead", false],
      ["web-worker", true],
    ]);
    expect(web?.rows[0]?.bound).toEqual({ kind: "text", text: "2 tasks" });
    expect(web?.rows[1]).toMatchObject({
      role: "Task worker",
      roleLine: "Task worker",
      asks: true,
      model: "-",
      bound: { kind: "ids", text: "T-12 · ws-7, T-13", batch: null },
      step: { tool: "Edit", text: "", faint: false },
    });
    expect(api?.label).toEqual({ name: "api", color: "stone" });
    expect(api?.rows).toEqual([]);
    expect(api?.profile).toEqual({
      facts: ["model sonnet"],
      instructions: null,
    });
    expect(web?.profile).toEqual({ facts: [], instructions: null });
    expect(main?.profile.facts).toEqual([]);
    expect(review?.profile.facts).toEqual([]);
    expect(review?.rows[0]?.bound).toEqual({
      kind: "text",
      text: "2 proposals",
    });
    expect(review?.rows[1]?.bound).toEqual({
      kind: "ids",
      text: "rb-3 · P-3, P-4",
      batch: "rb-3",
    });
    expect(web && areaNote(web)).toBe("area web · lead owns 2 tasks");
    expect(hasNoLeads(groups)).toBe(false);
    expect(isEmptyTeam(groups)).toBe(false);
    expect(mainAgentName(groups)).toBe("main");
  });

  it("words the bound column of workers with nothing bound", () => {
    const groups = teamGroups(
      input({
        workers: workerList({
          workers: [
            workerProcess("lost-lead", { area: "gone" }),
            workerProcess("lost-worker", { role: "task_worker", area: null }),
            workerProcess("review", { role: "review_lead", area: null }),
            workerProcess("review-b9", {
              role: "reviewer",
              area: null,
              proposals: ["P-7"],
            }),
            workerProcess("review-b8", { role: "reviewer", area: null }),
          ],
        }),
      }),
    );
    const rows = groups.flatMap((g) => g.rows);
    const bound = (name: string) => rows.find((r) => r.name === name)?.bound;
    expect(bound("lost-lead")).toEqual({ kind: "text", text: "-" });
    expect(bound("lost-worker")).toEqual({ kind: "text", text: "-" });
    expect(bound("review")).toEqual({ kind: "text", text: "-" });
    expect(bound("review-b9")).toEqual({
      kind: "ids",
      text: "P-7",
      batch: null,
    });
    expect(bound("review-b8")).toEqual({ kind: "text", text: "-" });
    const unnamed = groups.find((g) => g.key === "area:");
    expect(unnamed?.area).toBeNull();
    expect(unnamed?.label).toBeNull();
    expect(unnamed && areaNote(unnamed)).toBe("area");
    expect(unnamed?.rows.map((r) => r.child)).toEqual([true]);
  });

  it("puts a task worker another worker started under the last lead", () => {
    const groups = teamGroups(
      input({
        workers: workerList({
          areas: [areaEntry("web", "web", { lead: "b" })],
          workers: [
            workerProcess("a", { state: "stopped" }),
            workerProcess("b"),
            workerProcess("c", { role: "task_worker", started_by: "main" }),
            workerProcess("d", { role: "task_worker", started_by: "a" }),
            workerProcess("e", { role: "task_worker", started_by: "a/sub" }),
          ],
        }),
      }),
    );
    expect(groups[0]?.rows.map((r) => r.name)).toEqual([
      "a",
      "d",
      "e",
      "b",
      "c",
    ]);
    expect(groups[0] && areaNote(groups[0])).toBe("area · lead owns 0 tasks");
  });

  it("tells an empty team and a team without leads apart", () => {
    expect(isEmptyTeam(teamGroups(input()))).toBe(true);
    const onlyMain = teamGroups(
      input({
        agents: [agent("main", { presence: "offline" })],
        workers: workerList({
          areas: [areaEntry("web", "web")],
        }),
      }),
    );
    expect(isEmptyTeam(onlyMain)).toBe(false);
    expect(hasNoLeads(onlyMain)).toBe(true);
    expect(onlyMain[0]?.rows[0]?.bound).toEqual({ kind: "text", text: "-" });
    expect(onlyMain[0]?.rows[0]?.pill.label).toBe("offline");
    expect(mainAgentName([])).toBe("main");
  });

  it("finds the row of the worker that asks a question", () => {
    const questions = [permissionQuestion({ id: "pq-3", worker: "web-lead" })];
    const groups = teamGroups(
      input({
        questions,
        workers: workerList({ workers: [workerProcess("web-lead")] }),
      }),
    );
    expect(askingRow(groups, questions, "pq-3")?.name).toBe("web-lead");
    expect(askingRow(groups, questions, "pq-9")).toBeNull();
  });
});
