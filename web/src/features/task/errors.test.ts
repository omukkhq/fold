import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { describeError, fieldError } from "./errors";

const refusal = (code: string, details: Record<string, unknown> = {}) =>
  new ApiError(code, "the message must not matter", details);

describe("describeError", () => {
  it("renders the cycle path as ids", () => {
    expect(
      describeError(
        refusal("dependency_cycle", { path: ["T-1", "T-2", "T-1"] }),
      ),
    ).toEqual({
      field: null,
      text: "The edge would close a cycle: T-1 → T-2 → T-1.",
    });
  });

  it("falls back to a plain cycle text without a path", () => {
    expect(describeError(refusal("dependency_cycle"))?.text).toBe(
      "The dependency would close a cycle.",
    );
  });

  it("shows an existing edge as a banner", () => {
    expect(describeError(refusal("dependency_exists"))).toEqual({
      field: null,
      text: "The task already needs that task.",
    });
  });

  it("puts a label refusal on its field", () => {
    expect(describeError(refusal("label_exists"))?.field).toBe("name");
    expect(describeError(refusal("invalid_label_color"))?.field).toBe("color");
  });

  it("tells an invalid request apart by its reason", () => {
    for (const reason of [
      "self_dependency",
      "checklist_origin",
      "linked_item",
      "proposal_open",
      "two_areas",
    ]) {
      const view = describeError(refusal("invalid_request", { reason }));
      expect(view?.field, reason).toBeNull();
      expect(view?.text.length, reason).toBeGreaterThan(10);
    }
    expect(
      describeError(refusal("invalid_request", { reason: "self_dependency" }))
        ?.text,
    ).not.toBe(
      describeError(refusal("invalid_request", { reason: "linked_item" }))
        ?.text,
    );
  });

  it("says a task keeps one area when the daemon refuses a second", () => {
    expect(
      describeError(
        refusal("invalid_request", {
          reason: "two_areas",
          areas: ["web", "api"],
        }),
      )?.text,
    ).toBe(
      "A task can have only one area label. Keep one area, or let the main agent choose it.",
    );
  });

  it("leaves an invalid request with no known reason to the generic banner", () => {
    expect(describeError(refusal("invalid_request"))).toBeNull();
    expect(
      describeError(refusal("invalid_request", { reason: "other" })),
    ).toBeNull();
    expect(describeError(refusal("invalid_request", { reason: 3 }))).toBeNull();
  });

  it("names a closed task by its reason", () => {
    expect(
      describeError(refusal("task_closed", { reason: "done" }))?.text,
    ).toBe("The task is Done and takes no more changes.");
    expect(
      describeError(refusal("task_closed", { reason: "archived" }))?.text,
    ).toBe("The task is archived and takes no more changes.");
    expect(describeError(refusal("task_closed"))?.text).toBe(
      "The task is closed and takes no more changes.",
    );
    expect(describeError(refusal("task_closed", { reason: "x" }))?.text).toBe(
      "The task is closed and takes no more changes.",
    );
  });

  it("names a lead check that has nothing to check", () => {
    expect(describeError(refusal("task_not_reported"))?.text).toBe(
      "No report waits for a check. The task worker reports the task again first.",
    );
    expect(describeError(refusal("task_kind_mismatch"))?.field).toBeNull();
    expect(describeError(refusal("invalid_hold_reason"))?.text).toBe(
      "Give the blocker a reason of 1 to 500 characters.",
    );
    expect(describeError(refusal("prerequisites_not_done"))?.text).toBe(
      "A task that this task needs is not Done. Complete this task when its prerequisites are Done.",
    );
  });

  it("words the codes every screen words the same way from the shared sentences", () => {
    expect(describeError(refusal("human_name_required"))).toEqual({
      field: null,
      text: "Tell fold your name first, then try once more.",
    });
  });

  it("leaves every other code, and a value with no code, to the generic banner", () => {
    expect(describeError(refusal("task_not_found"))).toBeNull();
    expect(describeError(refusal("human_review_already_set"))).toBeNull();
    expect(describeError(new Error("x"))).toBeNull();
    expect(describeError(null)).toBeNull();
  });
});

describe("fieldError", () => {
  it("answers only for the field the refusal belongs to", () => {
    const error = refusal("label_exists");
    expect(fieldError(error, "name")).toBe("A label with this name exists.");
    expect(fieldError(error, "color")).toBeUndefined();
    expect(fieldError(refusal("dependency_exists"), "name")).toBeUndefined();
  });
});
