import { describe, expect, it } from "vitest";
import { NAV_ENTRIES, activeNav, navPath, switchProjectPath } from "./nav";

describe("NAV_ENTRIES", () => {
  it("lists the entries in top bar order", () => {
    expect(NAV_ENTRIES.map((e) => e.label)).toEqual([
      "Board",
      "Activity",
      "Proposals",
      "Requests",
      "Agents",
      "Messages",
      "Repository",
      "Settings",
    ]);
  });
});

describe("navPath", () => {
  it("builds the project path of an entry", () => {
    const entry = NAV_ENTRIES[2];
    expect(entry && navPath("demo", entry)).toBe("/p/demo/proposals");
  });
});

describe("activeNav", () => {
  it.each([
    ["/p/demo/board", "board"],
    ["/p/demo/board/", "board"],
    ["/p/demo/t/T-12", "board"],
    ["/p/demo/activity", "activity"],
    ["/p/demo/proposals", "proposals"],
    ["/p/demo/proposals/P-3", "proposals"],
    ["/p/demo/batches/rb-3", "proposals"],
    ["/p/demo/requests", "requests"],
    ["/p/demo/requests/RQ-4", "requests"],
    ["/p/demo/agents", "agents"],
    ["/p/demo/messages", "messages"],
    ["/p/demo/repository", "repository"],
    ["/p/demo/settings", "settings"],
  ])("%s selects %s", (path, id) => {
    expect(activeNav(path)).toBe(id);
  });

  it.each([
    "/",
    "/projects",
    "/sign-in",
    "/p/demo",
    "/p/demo/",
    "/p/demo/diff",
    "/p/demo/nope",
  ])("%s selects nothing", (path) => {
    expect(activeNav(path)).toBeNull();
  });
});

describe("switchProjectPath", () => {
  it.each([
    ["/p/demo/proposals/P-3", "/p/ledger/proposals"],
    ["/p/demo/requests/RQ-4", "/p/ledger/requests"],
    ["/p/demo/t/T-12", "/p/ledger/board"],
    ["/p/demo/batches/rb-3", "/p/ledger/proposals"],
    ["/p/demo/diff", "/p/ledger/board"],
    ["/p/demo", "/p/ledger/board"],
  ])("%s goes to %s", (path, target) => {
    expect(switchProjectPath(path, "ledger")).toBe(target);
  });
});
