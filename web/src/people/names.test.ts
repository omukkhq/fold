import { describe, expect, it } from "vitest";
import {
  agentNames,
  bareName,
  isHumanName,
  personKind,
  personOf,
  reviewerActor,
} from "./names";

const people = {
  humans: ["tanim", "frontend"],
  agents: ["frontend", "review"],
};

describe("actor names", () => {
  it("tells a human from an agent by the prefix", () => {
    expect(isHumanName("human:tanim")).toBe(true);
    expect(isHumanName("frontend/ui")).toBe(false);
    expect(personKind("human:tanim")).toBe("human");
    expect(personKind("fold")).toBe("agent");
  });

  it("shows a human without the prefix and an agent as it is", () => {
    expect(bareName("human:tanim")).toBe("tanim");
    expect(bareName("frontend/ui")).toBe("frontend/ui");
  });
});

describe("personOf", () => {
  it("finds a known name, with a human before an agent", () => {
    expect(personOf(people, "tanim")).toBe("human");
    expect(personOf(people, "review")).toBe("agent");
    expect(personOf(people, "frontend")).toBe("human");
  });

  it("refuses an unknown name, an empty one and a trailing slash", () => {
    expect(personOf(people, "nobody")).toBeNull();
    expect(personOf(people, "")).toBeNull();
    expect(
      personOf({ humans: [], agents: ["frontend/"] }, "frontend/"),
    ).toBeNull();
  });
});

describe("agentNames", () => {
  it("lists each agent and worker once", () => {
    expect(
      agentNames([
        {
          name: "frontend",
          workers: [{ name: "frontend" }, { name: "frontend/ui" }],
        },
        { name: "review", workers: [] },
      ]),
    ).toEqual(["frontend", "frontend/ui", "review"]);
  });
});

describe("reviewerActor", () => {
  it("turns a bare reviewer name into a human actor", () => {
    expect(reviewerActor("tanim")).toBe("human:tanim");
    expect(reviewerActor(null)).toBeNull();
  });
});
