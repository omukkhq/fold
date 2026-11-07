import { describe, expect, it } from "vitest";
import type { NeedsYouItem } from "@/api/workers";
import { needsItem } from "@/test/workers";
import {
  folderActions,
  dialogPaths,
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
      "fold first stashes your unrecorded changes as stash@{0}, so fld unstash brings them back. Then the project folder matches main again. Your change that waits for the main agent is withdrawn: its files, with the conflict in 1 file, go into stash@{0} too. If the main agent opened a workspace to merge it, fold closes that workspace.",
    );
    expect(dialogPaths("reset", actions)).toEqual({
      listed: ["plan.md"],
      rest: "and your other unrecorded changes",
    });
  });

  it("reset a held folder, drop the conflicted stash entry and keep its files with markers, and restore the held files", () => {
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
    expect(dialogPaths("reset", actions)).toEqual({
      listed: ["a", "b"],
      rest: "and your other unrecorded changes",
    });
    expect(dialogPaths("restore", actions)).toEqual({
      listed: ["a", "b"],
      rest: null,
    });
    expect(restoreText(["a", "b"])).toEqual({
      title: "Restore 2 files?",
      description:
        "fold first stashes the changes of these files as stash@{0}, so fld unstash brings them back. Then these files match main again; your other changes stay.",
    });
  });

  it("name the first held files of a restore and count the others", () => {
    const paths = (count: number) =>
      Array.from({ length: count }, (_, index) => `f${String(index)}`);
    const restore = (count: number) => {
      const actions = folderActions([item("folder_held", paths(count))]);
      if (!actions) throw new Error("actions");
      return dialogPaths("restore", actions);
    };
    expect(restore(9)).toEqual({ listed: paths(9), rest: null });
    expect(restore(10).rest).toBe("and 1 other file");
    expect(restore(12)).toEqual({
      listed: paths(9),
      rest: "and 3 other files",
    });
  });
});
