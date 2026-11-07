import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import {
  LISTED_PATHS,
  agentsText,
  confirmationOf,
  dialogText,
  failureAt,
  folderWriteKey,
  nextAsking,
  putAsideLabel,
  settleFailure,
  type Asking,
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

const change = { paths: ["README.md", "junk1.txt"], agents: ["reviewer-35"] };

function refusal(code: string, reason?: string): ApiError {
  return new ApiError(code, "refused", reason === undefined ? {} : { reason });
}

describe("the choices of a worker's change", () => {
  it("names one, two or many agents", () => {
    expect(agentsText([])).toBe("an agent");
    expect(agentsText(["reviewer-35"])).toBe("reviewer-35");
    expect(agentsText(["backend", "web-lead"])).toBe("backend and web-lead");
    expect(agentsText(["a", "b", "c"])).toBe("3 agents");
    expect(putAsideLabel(["reviewer-35"])).toBe("Put the agent's files aside");
    expect(putAsideLabel(["a", "b"])).toBe("Put the agents' files aside");
  });

  it("asks before it puts files aside or keeps them, with the files the person saw", () => {
    const restore = nextAsking(null, {
      type: "ask",
      action: "restore",
      change,
    });
    expect(restore).toEqual({ action: "restore", change });
    const text = restore === null ? null : dialogText(restore);
    expect(text).toMatchObject({
      title: "Put 2 files aside?",
      confirm: "Put files aside",
      danger: false,
      notice: null,
    });
    expect(text?.description).toContain("in stash@{0}");
    expect(text?.description).toContain("Your other changes stay.");
    expect(text?.description).toContain("fld unstash brings them back.");
    const keep = dialogText({
      action: "ignore",
      change: { paths: ["a"], agents: ["reviewer-35"] },
    });
    expect(keep).toMatchObject({
      title: "Keep 1 file as yours?",
      confirm: "Keep as mine",
      danger: false,
    });
    expect(keep.description).toContain("a change of reviewer-35");
  });

  it("opens the record dialog on a confirmation and says when the folder changed since", () => {
    const first = nextAsking(null, {
      type: "failed",
      error: heldBack({ tree: "abc", emptied: ["README.md"] }),
    });
    expect(first).toMatchObject({ action: "record", changed: false });
    const text = first === null ? null : dialogText(first);
    expect(text).toMatchObject({ confirm: "Record anyway", danger: true });
    expect(text?.description).toContain(
      "This records the whole project folder on main, also your other unrecorded changes.",
    );
    const same = nextAsking(first, {
      type: "failed",
      error: heldBack({ tree: "abc", emptied: ["README.md"] }),
    });
    expect(same).toMatchObject({ changed: false });
    const moved = nextAsking(first, {
      type: "failed",
      error: heldBack({ tree: "def", emptied: ["plan.md"] }),
    });
    expect(moved).toMatchObject({ action: "record", changed: true });
    expect(moved === null ? null : dialogText(moved).notice).toBe(
      "The folder changed; check the files again.",
    );
  });

  it("keeps the open dialog on any other refusal and closes on success or close", () => {
    const open: Asking = { action: "restore", change };
    expect(
      nextAsking(open, { type: "failed", error: refusal("git_failed") }),
    ).toBe(open);
    expect(
      nextAsking(null, { type: "failed", error: refusal("git_failed") }),
    ).toBeNull();
    expect(nextAsking(open, { type: "done" })).toBeNull();
    expect(nextAsking(open, { type: "close" })).toBeNull();
  });

  it("shows a refusal in the open dialog, else under the line, and never the confirmation", () => {
    const open: Asking = { action: "ignore", change };
    const failed = refusal("agent_change_not_found");
    expect(failureAt(open, failed, "dialog")).toBe(
      "No change of an agent waits any more.",
    );
    expect(failureAt(open, failed, "line")).toBeNull();
    expect(failureAt(null, failed, "line")).toBe(
      "No change of an agent waits any more.",
    );
    expect(failureAt(null, failed, "dialog")).toBeNull();
    expect(failureAt(open, null, "dialog")).toBeNull();
    expect(failureAt(open, heldBack({ tree: "abc" }), "dialog")).toBeNull();
  });

  it("words each refusal by its code and reason", () => {
    expect(settleFailure(heldBack({ tree: "abc" }))).toBeNull();
    const reasons: [string, string][] = [
      ["git_commits_wait", "commits that fold did not make"],
      ["head_moved", "moved outside fold"],
      ["stash_conflicted", "The conflict of an unstash waits"],
      ["stash_markers", "conflict markers of an unstash"],
      ["main_changed", "Main changed these files"],
      ["changed_during_stash", "changed while fold put the files aside"],
    ];
    for (const [reason, words] of reasons) {
      expect(
        settleFailure(refusal("invalid_request", reason)),
        reason,
      ).toContain(words);
    }
    expect(settleFailure(refusal("invalid_request", "other"))).toBe(
      "The daemon refused the request as written.",
    );
    expect(settleFailure(refusal("invalid_request"))).toBe(
      "The daemon refused the request as written.",
    );
    expect(settleFailure(refusal("head_moved"))).toBe(
      "Main moved during every try to record. Try again.",
    );
    expect(settleFailure(refusal("git_failed"))).toContain(
      "A Git command failed",
    );
    expect(settleFailure(refusal("worker_peer_refused"))).toContain(
      "a worker or a check",
    );
    for (const code of ["client_header_required", "host_not_allowed"]) {
      expect(settleFailure(refusal(code)), code).toContain("its own address");
    }
    expect(settleFailure(refusal("human_name_required"))).toBe(
      "Tell fold your name first, then try once more.",
    );
    expect(settleFailure(new ApiError("internal_error", "x"))).toBe(
      "The daemon failed. Try again.",
    );
    expect(settleFailure(new Error("offline"))).toBe(
      "The request did not reach the daemon. Try again.",
    );
  });

  it("shares one mutation key with every write of the project folder", () => {
    expect(folderWriteKey("demo")).toEqual(["demo", "folder-write"]);
  });
});
