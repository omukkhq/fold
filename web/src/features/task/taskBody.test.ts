import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { taskBody } from "./taskBody";

const task = { id: "T-2" };
const failed = new ApiError("internal_error", "the message must not matter");
const missing = new ApiError("task_not_found", "the message must not matter");

describe("taskBody", () => {
  it("loads until the first read answers", () => {
    expect(taskBody(undefined, null)).toEqual({ kind: "loading" });
  });

  it("shows the task the daemon sent", () => {
    expect(taskBody(task, null)).toEqual({
      kind: "loaded",
      task,
      failure: null,
    });
  });

  it("keeps the task when a read again fails", () => {
    expect(taskBody(task, failed)).toEqual({
      kind: "loaded",
      task,
      failure: failed,
    });
  });

  it("shows the full error state only when no read returned a task", () => {
    expect(taskBody(undefined, failed)).toEqual({ kind: "failed" });
    expect(taskBody(undefined, new Error("offline"))).toEqual({
      kind: "failed",
    });
  });

  it("shows a missing task as missing, also over a task read earlier", () => {
    expect(taskBody(undefined, missing)).toEqual({ kind: "missing" });
    expect(taskBody(task, missing)).toEqual({ kind: "missing" });
  });
});
