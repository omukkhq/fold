import { describe, expect, it } from "vitest";
import { areaEntry } from "@/test/workers";
import {
  NO_AREA,
  UNMARKED,
  areaChanges,
  areaOptions,
  createLabels,
  labelChoices,
  labelControls,
  labelNamed,
  leadText,
  markedArea,
  swapArea,
  taskLabelView,
} from "./areas";

const frontend = { name: "frontend", color: "sage" };
const backend = { name: "backend", color: "gold" };
const bug = { name: "bug", color: "plum" };
const database = { name: "database", color: "slate" };
const labels = [backend, bug, database, frontend];
const areas = [
  areaEntry("api", "backend", { lead: "api-lead" }),
  areaEntry("web", "frontend", { lead: "web-lead" }),
];

describe("area options", () => {
  it("lists every area with its label colour and lead, in the daemon's order", () => {
    expect(areaOptions(areas, labels)).toEqual([
      { name: "api", label: backend, lead: "api-lead" },
      { name: "web", label: frontend, lead: "web-lead" },
    ]);
  });

  it("draws an area whose label is not in the list in stone", () => {
    expect(areaOptions([areaEntry("docs", "writing")], labels)).toEqual([
      { name: "docs", label: { name: "writing", color: "stone" }, lead: null },
    ]);
  });

  it("says an area has no lead before its first lead starts", () => {
    expect(leadText("web-lead")).toBe("web-lead");
    expect(leadText(null)).toBe("no lead yet");
  });
});

describe("a task's labels", () => {
  it("splits the area from the other labels", () => {
    const view = taskLabelView([bug, frontend], areas);
    expect(view.areas.map((a) => a.name)).toEqual(["web"]);
    expect(view.others).toEqual([bug]);
    expect(markedArea(view, false)).toBe("web");
  });

  it("marks no area for a task without one", () => {
    const view = taskLabelView([bug], areas);
    expect(view.areas).toEqual([]);
    expect(markedArea(view, false)).toBe(NO_AREA);
  });

  it("marks no choice for a task that holds two", () => {
    const view = taskLabelView([frontend, backend], areas);
    expect(view.areas.map((a) => a.name)).toEqual(["api", "web"]);
    expect(view.others).toEqual([]);
    expect(markedArea(view, false)).toBe(UNMARKED);
  });

  it("marks no choice when the daemon flags two areas that the area list does not show", () => {
    const view = taskLabelView([frontend], areas);
    expect(markedArea(view, true)).toBe(UNMARKED);
    expect(markedArea(taskLabelView([], areas), true)).toBe(UNMARKED);
  });

  it("finds a label by name, and draws one missing from the list in stone", () => {
    expect(labelNamed(labels, "bug")).toEqual(bug);
    expect(labelNamed(labels, "gone")).toEqual({
      name: "gone",
      color: "stone",
    });
  });
});

describe("label choices", () => {
  it("offers the labels that are no area and not chosen", () => {
    expect(labelChoices(labels, areas, ["bug"], "")).toEqual([database]);
  });

  it("filters by a part of the name, in any case, ignoring spaces around it", () => {
    expect(labelChoices(labels, areas, [], " DATA ")).toEqual([database]);
    expect(labelChoices(labels, areas, [], "front")).toEqual([]);
  });
});

describe("creating a task", () => {
  it("puts the chosen area's label first", () => {
    expect(createLabels(areas, "web", ["bug"])).toEqual(["frontend", "bug"]);
  });

  it("sends only the other labels when the main agent chooses the area", () => {
    expect(createLabels(areas, NO_AREA, ["bug", "bug"])).toEqual(["bug"]);
    expect(createLabels(areas, "gone", [])).toEqual([]);
  });
});

describe("changing the area", () => {
  it("swaps the area label and keeps the other labels", () => {
    expect(swapArea([bug, frontend], areas, "api")).toEqual(["bug", "backend"]);
  });

  it("removes every area label to let the main agent choose", () => {
    expect(swapArea([frontend, backend, bug], areas, NO_AREA)).toEqual(["bug"]);
  });

  it("writes only a pick that changes the task", () => {
    const one = taskLabelView([frontend], areas);
    expect(areaChanges(one, false, "web")).toBe(false);
    expect(areaChanges(one, false, "api")).toBe(true);
    expect(areaChanges(one, false, NO_AREA)).toBe(true);
    const none = taskLabelView([], areas);
    expect(areaChanges(none, false, NO_AREA)).toBe(false);
    const two = taskLabelView([frontend, backend], areas);
    expect(areaChanges(two, false, "web")).toBe(true);
    expect(areaChanges(two, false, NO_AREA)).toBe(true);
  });

  it("writes any pick on a task the daemon flags with two areas", () => {
    const one = taskLabelView([frontend], areas);
    expect(areaChanges(one, true, "web")).toBe(true);
    expect(areaChanges(one, true, NO_AREA)).toBe(true);
  });
});

describe("label controls", () => {
  it("take input while no label write runs", () => {
    expect(labelControls(false, false)).toEqual({ area: true, labels: true });
  });

  it("all wait while any label write runs, since the area write sends the whole set", () => {
    expect(labelControls(false, true)).toEqual({ area: false, labels: false });
    expect(labelControls(true, true)).toEqual({ area: false, labels: false });
  });

  it("leave only the area change on a task with two areas", () => {
    expect(labelControls(true, false)).toEqual({ area: true, labels: false });
  });
});
