import { describe, expect, it } from "vitest";
import type { NeedsYouItem } from "@/api/workers";
import { needsItem } from "@/test/workers";
import { folderActions, resetText, restoreText } from "./folderReset";

function item(kind: NeedsYouItem["kind"], paths: string[]): NeedsYouItem {
  return needsItem({ kind, paths });
}

describe("the folder actions", () => {
  it("are absent while nothing waits on the project folder", () => {
    expect(folderActions([])).toBeNull();
    expect(folderActions([item("blocker", [])])).toBeNull();
  });

  it("drop a waiting change after a safety stash", () => {
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
      "fold first stashes your unrecorded changes as stash@{0}, so Unstash brings them back. Then the project folder matches main again. Your change that waits for the main agent is dropped, with its conflict in 1 file, and fold abandons the merge workspace the main agent made for it.",
    );
  });

  it("reset a held folder and end an unstash conflict, and restore the held files", () => {
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
      "The unstash conflict in 1 file ends; stash@{0} keeps those files with their markers.",
    );
    expect(restoreText(["a", "b"])).toEqual({
      title: "Restore 2 files?",
      description:
        "fold first stashes their changes as stash@{0}, so Unstash brings them back. Then these files match main again; your other changes stay.",
    });
  });
});
