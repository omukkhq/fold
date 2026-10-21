import { describe, expect, it } from "vitest";
import type { WorkerProcess } from "@/api/workers";
import { message, recipient, thread } from "@/test/messages";
import {
  STOPPED_NOTE,
  isStopped,
  replyAgents,
  stoppedReplyNote,
  stoppedTaskWorkers,
} from "./stopped";

const worker = (
  name: string,
  role: WorkerProcess["role"],
  state: WorkerProcess["state"],
): Pick<WorkerProcess, "name" | "role" | "state"> => ({ name, role, state });

describe("stopped task workers", () => {
  it("holds the stopped and failed task workers only", () => {
    const stopped = stoppedTaskWorkers([
      worker("web-worker", "task_worker", "stopped"),
      worker("api-worker", "task_worker", "failed"),
      worker("ui-worker", "task_worker", "idle"),
      worker("web-lead", "area_lead", "stopped"),
      worker("reviewer-1", "reviewer", "stopped"),
    ]);
    expect([...stopped]).toEqual(["web-worker", "api-worker"]);
    expect(stoppedTaskWorkers(undefined).size).toBe(0);
  });

  it("knows a worker of a stopped agent by its first segment", () => {
    const stopped = new Set(["web-worker"]);
    expect(isStopped("web-worker", stopped)).toBe(true);
    expect(isStopped("web-worker/ui", stopped)).toBe(true);
    expect(isStopped("web-lead", stopped)).toBe(false);
  });

  it("notes the stopped reply targets for the reply box", () => {
    const stopped = new Set(["web-worker"]);
    expect(stoppedReplyNote(["web-worker", "web-lead"], stopped)).toBe(
      `@web-worker: ${STOPPED_NOTE}`,
    );
    expect(stoppedReplyNote(["web-lead"], stopped)).toBe(null);
    expect(stoppedReplyNote([], stopped)).toBe(null);
  });

  it("finds the agents a reply reaches from the newest message", () => {
    const asked = message("m-1", {
      sender: "human:tanim",
      recipients: [recipient("web-worker"), recipient("human:ana")],
    });
    expect(replyAgents(thread("m-1", { messages: [asked] }))).toEqual([
      "web-worker",
    ]);
    const note = message("m-2", {
      sender: "fold",
      recipients: [recipient("human:tanim"), recipient("web-lead")],
    });
    expect(replyAgents(thread("m-1", { messages: [asked, note] }))).toEqual([
      "web-lead",
    ]);
    const answer = message("m-3", { sender: "web-lead/ui" });
    expect(
      replyAgents(thread("m-1", { messages: [asked, note, answer] })),
    ).toEqual(["web-lead/ui"]);
    expect(replyAgents(thread("m-1", { messages: [] }))).toEqual([]);
  });
});
