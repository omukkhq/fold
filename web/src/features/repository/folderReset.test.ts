import { describe, expect, it } from "vitest";
import type { NeedsYouItem } from "@/api/workers";
import { needsItem } from "@/test/workers";
import {
  folderActions,
  resetPaths,
  resetText,
  restoreText,
} from "./folderReset";

function item(kind: NeedsYouItem["kind"], paths: string[]): NeedsYouItem {
  return needsItem({ kind, paths });
}

describe("the folder actions", () => {
  it("are absent while nothing waits on the project folder", () => {
    expect(folderActions([])).toBeNull();
    expect(folderActions([item("blocker", [])])).toBeNull();
  });

  it("withdraw a waiting change after a safety stash that holds its files, and close the main agent's merge workspace", () => {
    const actions = folderActions([item("human_change", ["plan.md"])]);
    expect(actions).toEqual({
      heldChange: ["plan.md"],
      heldPaths: [],
      markers: [],
    });
    if (!actions) throw new Error("actions");
    const text = resetText(actions);
    expect(text.label).toBe("Drop my change");
    expect(text.title).toBe("Drop your waiting change?");
    expect(text.description).toBe(
      "fold first stashes your unrecorded changes as stash@{0}, so fld unstash brings them back. Then the project folder matches main again. Your change that waits for the main agent is withdrawn: its files, with the conflict in 1 file, go into stash@{0} too, and fold closes the workspace the main agent opened to merge it.",
    );
    expect(resetPaths(actions)).toEqual(["plan.md"]);
  });

  it("reset a held folder, drop the conflicted stash entry and keep its files with markers, and restore the held files without closing a workspace", () => {
    const actions = folderActions([
      item("folder_held", ["a", "b"]),
      item("unstash_conflicted", ["c"]),
    ]);
    if (!actions) throw new Error("actions");
    expect(actions.heldPaths).toEqual(["a", "b"]);
    const text = resetText(actions);
    expect(text.label).toBe("Reset folder");
    expect(text.title).toBe("Reset the project folder?");
    expect(text.description).toContain(
      "The unstash conflict in 1 file ends: fold drops the stash entry that conflicted, and stash@{0} keeps its files as they are now, with the conflict markers.",
    );
    expect(text.description).not.toContain("workspace");
    expect(resetPaths(actions)).toEqual(["a", "b"]);
    expect(restoreText(["a", "b"])).toEqual({
      title: "Restore 2 files?",
      description:
        "fold first stashes the changes of these files as stash@{0}, so fld unstash brings them back. Then these files match main again; your other changes stay. A restore never closes a workspace of the main agent; only a reset does.",
    });
  });
});
