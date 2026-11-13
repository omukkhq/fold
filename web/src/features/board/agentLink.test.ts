import { describe, expect, it } from "vitest";
import { agent } from "@/test/agents";
import { areaEntry, workerList } from "@/test/workers";
import { agentHref, agentLinksOf, NO_AGENT_LINKS } from "./agentLink";

const links = agentLinksOf(
  workerList({
    areas: [
      areaEntry("api", "backend", { lead: "api-lead" }),
      areaEntry("web", "frontend"),
    ],
  }),
  [agent("api-lead"), agent("main", { tags: ["main"] }), agent("api-t3")],
);
const board = (owner: string) =>
  `/p/demo/board?owner=${encodeURIComponent(JSON.stringify([owner]))}`;

describe("agentLinksOf", () => {
  it("maps each started lead to its area and finds the main agent by its tag", () => {
    expect([...links.leads]).toEqual([["api-lead", "api"]]);
    expect([...links.main]).toEqual(["main"]);
  });

  it("knows no lead and no main agent before the answers arrive", () => {
    expect(agentLinksOf(undefined, undefined)).toEqual(NO_AGENT_LINKS);
  });
});

describe("agentHref", () => {
  it("opens a lead's area on the board with its Owner entry ticked", () => {
    expect(agentHref("demo", "api-lead", links)).toBe(board("area:api"));
    expect(agentHref("demo", "api-lead/old", links)).toBe(board("area:api"));
  });

  it("opens the main agent's tasks on the board with Main agent ticked", () => {
    expect(agentHref("demo", "main", links)).toBe(board("main"));
  });

  it("opens the Agents screen for any other agent", () => {
    for (const name of ["api-t3", "review-lead", "reviewer-9"]) {
      expect(agentHref("demo", name, links)).toBe("/p/demo/agents");
    }
    expect(agentHref("demo", "api-lead", NO_AGENT_LINKS)).toBe(
      "/p/demo/agents",
    );
  });
});
