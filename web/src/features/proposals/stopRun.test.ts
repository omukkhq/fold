import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { checkRun } from "@/test/proposals";
import { showsStop, stopFailure, stopPending } from "./stopRun";

describe("showsStop", () => {
  it("shows for a running run, which also covers a run that waits for memory", () => {
    expect(showsStop(checkRun(1, { state: "running" }), "tanim")).toBe(true);
    expect(
      showsStop(
        checkRun(1, { state: "running", waiting_for_memory_since: 5 }),
        "tanim",
      ),
    ).toBe(true);
  });

  it("hides for a finished run", () => {
    for (const state of ["passed", "failed", "stopped"] as const) {
      expect(showsStop(checkRun(1, { state }), "tanim")).toBe(false);
    }
  });

  it("hides for an integration run, which the daemon never cancels", () => {
    expect(
      showsStop(checkRun(1, { state: "running", stage: "integration" }), "t"),
    ).toBe(false);
  });

  it("hides when no human is signed in", () => {
    expect(showsStop(checkRun(1, { state: "running" }), null)).toBe(false);
  });
});

describe("stopPending", () => {
  it("is pending while sending and after the daemon accepted, until the run ends", () => {
    expect(stopPending({ sending: true, accepted: false })).toBe(true);
    expect(stopPending({ sending: false, accepted: true })).toBe(true);
    expect(stopPending({ sending: false, accepted: false })).toBe(false);
  });
});

describe("stopFailure", () => {
  it("words each known code", () => {
    expect(stopFailure(new ApiError("check_run_ended", "x")).text).toMatch(
      /ended before/,
    );
    expect(stopFailure(new ApiError("check_run_not_found", "x")).text).toMatch(
      /no check run/,
    );
    expect(stopFailure(new ApiError("invalid_request", "x")).code).toBe(
      "invalid_request",
    );
  });

  it("uses the shared sentence for a common code and a plain one otherwise", () => {
    expect(stopFailure(new ApiError("human_name_required", "m")).text).toMatch(
      /your name/,
    );
    expect(stopFailure(new Error("offline"))).toEqual({
      code: null,
      text: "The request did not reach the daemon. Try again.",
    });
  });

  it("never reads the message", () => {
    expect(
      stopFailure(new ApiError("check_run_ended", "something else")).text,
    ).toMatch(/ended before/);
  });
});
