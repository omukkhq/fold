import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { workerProcess } from "@/test/workers";
import { restartBody, restartFailure } from "./restart";
import { canRestart, roleLine } from "./team";

describe("restartBody", () => {
  it("leaves out empty instructions", () => {
    expect(restartBody("  ")).toEqual({});
    expect(restartBody(" Plan again. ")).toEqual({
      instructions: "Plan again.",
    });
  });
});

describe("restartFailure", () => {
  it("words each refusal for its code", () => {
    for (const [code, start] of [
      ["project_paused", "The agents of this project are paused."],
      ["worker_limit_reached", "fold runs as many workers"],
      ["worker_exists", "The worker is starting in a new session"],
      ["worker_busy", "A stop came first"],
      ["worker_process_not_found", "fold runs no process"],
      ["invalid_config", "fold cannot read the worker settings"],
    ] as const) {
      const failure = restartFailure(new ApiError(code, ""));
      expect(failure.code).toBe(code);
      expect(failure.text.startsWith(start)).toBe(true);
    }
    expect(restartFailure(new ApiError("human_name_required", "")).code).toBe(
      "human_name_required",
    );
    expect(restartFailure(new Error("offline"))).toEqual({
      code: null,
      text: "The request did not reach the daemon. Try again.",
    });
  });
});

describe("canRestart", () => {
  it("allows every state but a stop under way", () => {
    expect(canRestart("working")).toBe(true);
    expect(canRestart("idle")).toBe(true);
    expect(canRestart("starting")).toBe(true);
    expect(canRestart("stopped")).toBe(true);
    expect(canRestart("failed")).toBe(true);
    expect(canRestart("stopping")).toBe(false);
  });
});

describe("roleLine", () => {
  const session = (id: string) => ({
    session_id: id,
    reason: "requested" as const,
    ended_at: 1,
  });

  it("counts the earlier sessions after the role", () => {
    expect(roleLine(workerProcess("web-lead"))).toBe("Area lead");
    expect(
      roleLine(
        workerProcess("web-lead", { previous_sessions: [session("s-1")] }),
      ),
    ).toBe("Area lead · 1 earlier session");
    expect(
      roleLine(
        workerProcess("web-lead", {
          previous_sessions: [session("s-1"), session("s-2")],
        }),
      ),
    ).toBe("Area lead · 2 earlier sessions");
  });
});
