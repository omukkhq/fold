import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import {
  STASH_CHANGED_TEXT,
  folderErrorText,
  stashChangedError,
} from "./errors";

function refused(reason: string, details: Record<string, unknown> = {}) {
  return new ApiError("invalid_request", "the daemon's words", {
    reason,
    ...details,
  });
}

describe("folderErrorText", () => {
  it("says the stash changed for the screen's own check and for both refusals of the daemon that mean it", () => {
    expect(folderErrorText(stashChangedError())).toBe(STASH_CHANGED_TEXT);
    expect(folderErrorText(refused("stash_changed"))).toBe(STASH_CHANGED_TEXT);
    expect(folderErrorText(refused("stash_entry_missing"))).toBe(
      STASH_CHANGED_TEXT,
    );
    expect(STASH_CHANGED_TEXT).toBe("The stash changed; check the list again.");
  });

  it("names the files of a waiting unstash conflict and of a tracked file fold cannot stash", () => {
    expect(
      folderErrorText(refused("stash_conflicted", { paths: ["a.md", "b.md"] })),
    ).toBe(
      "An unstash conflict waits in the project folder. Fix the files and run `fld record`, or reset the folder. Files: `a.md`, `b.md`.",
    );
    expect(
      folderErrorText(refused("skipped_tracked", { paths: ["big.bin", 3] })),
    ).toBe(
      "fold cannot stash a tracked file that is too large or unreadable. Make it smaller or readable, then try again. Files: `big.bin`.",
    );
    expect(folderErrorText(refused("stash_conflicted"))).toBe(
      "An unstash conflict waits in the project folder. Fix the files and run `fld record`, or reset the folder. Files:",
    );
  });

  it("gives the next step for each other refusal", () => {
    expect(folderErrorText(refused("changed_during_stash"))).toContain(
      "Files changed while fold stashed them",
    );
    expect(folderErrorText(refused("head_moved"))).toContain(
      "Put the branch back first",
    );
    expect(folderErrorText(refused("git_commits_wait"))).toContain(
      "Your Git commits wait for the main agent",
    );
    expect(folderErrorText(refused("no_paths"))).toBe(
      "No file is named to restore.",
    );
    expect(folderErrorText(refused("stash_empty"))).toBe(
      "The stash is empty now; nothing is dropped.",
    );
  });

  it("words a refusal by its code, and leaves an unknown one to the generic banner", () => {
    expect(folderErrorText(new ApiError("human_required", ""))).toBe(
      "Only a signed-in human can change the project folder or its stash.",
    );
    expect(folderErrorText(new ApiError("auth_required", ""))).toBe(
      "Sign in again, then try once more.",
    );
    expect(folderErrorText(refused("some_new_reason"))).toBeNull();
    expect(
      folderErrorText(new ApiError("invalid_request", "", { reason: 4 })),
    ).toBeNull();
    expect(folderErrorText(new ApiError("internal_error", ""))).toBeNull();
    expect(folderErrorText(new Error("network"))).toBeNull();
  });
});
