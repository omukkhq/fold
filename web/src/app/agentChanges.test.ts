import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import {
  ACTION_LABELS,
  LISTED_PATHS,
  PENDING_LABELS,
  agentsText,
  confirmationOf,
  settleFailure,
} from "./agentChanges";

function heldBack(details: Record<string, unknown>): ApiError {
  return new ApiError(
    "record_needs_confirmation",
    "the change empties 1 tracked file; nothing was recorded",
    details,
  );
}

describe("a record that waits for a confirmation", () => {
  it("names the tree to confirm and the files of each reason, counting the rest", () => {
    const changed = Array.from({ length: 25 }, (_, n) => `junk${String(n)}`);
    const confirmation = confirmationOf(
      heldBack({
        tree: "abc",
        emptied: ["README.md"],
        emptied_count: 1,
        empty_added: [],
        empty_added_count: 0,
        agent_changed: changed,
        agent_changed_count: 27,
      }),
    );
    expect(confirmation?.tree).toBe("abc");
    expect(confirmation?.groups.map((group) => group.key)).toEqual([
      "emptied",
      "agent_changed",
    ]);
    expect(confirmation?.groups[0]).toEqual({
      key: "emptied",
      title: "Emptied",
      paths: ["README.md"],
      more: 0,
    });
    expect(confirmation?.groups[1]?.paths).toHaveLength(LISTED_PATHS);
    expect(confirmation?.groups[1]?.more).toBe(27 - LISTED_PATHS);
  });

  it("counts the listed files when the refusal names no count, and skips what is not a path", () => {
    const confirmation = confirmationOf(
      heldBack({ tree: "abc", empty_added: ["a", 3, "b"] }),
    );
    expect(confirmation?.groups).toEqual([
      {
        key: "empty_added",
        title: "New and empty",
        paths: ["a", "b"],
        more: 0,
      },
    ]);
  });

  it("is no confirmation for any other refusal or a refusal without a tree", () => {
    expect(confirmationOf(new ApiError("head_moved", "moved"))).toBeNull();
    expect(confirmationOf(heldBack({}))).toBeNull();
    expect(confirmationOf(new Error("boom"))).toBeNull();
  });
});

describe("the choices of a worker's change", () => {
  it("names one, two or many agents", () => {
    expect(agentsText([])).toBe("an agent");
    expect(agentsText(["reviewer-35"])).toBe("reviewer-35");
    expect(agentsText(["backend", "web-lead"])).toBe("backend and web-lead");
    expect(agentsText(["a", "b", "c"])).toBe("3 agents");
  });

  it("words each refusal by its code and leaves the confirmation to the dialog", () => {
    expect(settleFailure(heldBack({ tree: "abc" }))).toBeNull();
    expect(settleFailure(new ApiError("agent_change_not_found", ""))).toBe(
      "No change of an agent waits any more.",
    );
    expect(settleFailure(new ApiError("internal_error", "x"))).toBe(
      "The daemon failed. Try again.",
    );
    expect(settleFailure(new Error("offline"))).toBe(
      "The request did not reach the daemon. Try again.",
    );
  });

  it("gives every choice its idle and its pending words", () => {
    expect(Object.keys(ACTION_LABELS)).toEqual(Object.keys(PENDING_LABELS));
    expect(PENDING_LABELS.record).toBe("Recording…");
  });
});
