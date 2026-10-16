import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { listAgents } from "./agents";
import { isApiError } from "./errors";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("listAgents", () => {
  it("unwraps the agents and asks for events only when told to", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/agents": {
        body: { agents: [{ name: "frontend" }] },
      },
    });
    const agents = await listAgents("demo");
    expect(agents.map((a) => a.name)).toEqual(["frontend"]);
    await listAgents("demo", 5);
    expect(calls[0]?.search.has("events")).toBe(false);
    expect(calls[1]?.search.get("events")).toBe("5");
  });

  it("rejects with the daemon's code", async () => {
    stubFetch({
      "GET /api/v1/projects/ghost/agents": errorReply(404, "project_not_found"),
    });
    await expect(listAgents("ghost")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "project_not_found"),
    );
  });
});
