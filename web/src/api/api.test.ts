import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { parseSlug } from "../routes/params";
import { stubFetch } from "../test/stubFetch";
import { RECENT_EVENTS } from "./agents";
import { agentBarQuery } from "./agentBar";
import { keys } from "./keys";
import {
  agentsQuery,
  messagesQuery,
  projectQuery,
  projectsQuery,
  sessionQuery,
} from "./queries";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("queries", () => {
  it("fetch through the api functions under the key builders", async () => {
    stubFetch({
      "GET /api/v1/projects": {
        body: { projects: [{ slug: "demo", name: "demo", available: true }] },
      },
      "GET /api/v1/projects/demo": {
        body: { slug: "demo", name: "demo", available: true },
      },
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/agents": { body: { agents: [] } },
      "GET /api/v1/projects/demo/messages": {
        body: { items: [], next: null },
      },
    });
    const client = new QueryClient();
    const [list, project, who, agents, threads] = await Promise.all([
      client.query(projectsQuery()),
      client.query(projectQuery("demo")),
      client.query(sessionQuery()),
      client.query(agentsQuery("demo")),
      client.query(messagesQuery("demo")),
    ]);
    expect(list.map((p) => p.slug)).toEqual(["demo"]);
    expect(project.slug).toBe("demo");
    expect(who.human).toBe("tanim");
    expect(agents).toEqual([]);
    expect(threads).toEqual({ items: [], more: false });
    expect(client.getQueryData(keys.project("demo"))).toEqual(project);
  });

  it("asks the Agents screen for recent events and the top bar for none", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/agents": { body: { agents: [] } },
    });
    const client = new QueryClient();
    await client.query(agentsQuery("demo"));
    await client.query(agentBarQuery("demo"));
    expect(calls.map((c) => c.search.get("events"))).toEqual([
      String(RECENT_EVENTS),
      null,
    ]);
  });
});

describe("keys", () => {
  it("starts every project key with the slug", () => {
    const projectKeys = [
      keys.project("s"),
      keys.board("s"),
      keys.task("s", "T-1"),
      keys.activity("s"),
      keys.proposals("s"),
      keys.proposal("s", "P-2"),
      keys.agents("s"),
      keys.messages("s"),
      keys.repository("s"),
      keys.workspaces("s"),
      keys.workspace("s", "ws-3"),
      keys.workspaceLive("s", "ws-3"),
      keys.workspaceLiveAll("s"),
      keys.events("s"),
      keys.reviewCheckouts("s"),
    ];
    for (const key of projectKeys) {
      expect(key[0]).toBe("s");
    }
  });

  it("reads the project list again on the viewer's next day, under the projects prefix", () => {
    const morning = projectsQuery(new Date(2026, 9, 21, 8).getTime()).queryKey;
    const evening = projectsQuery(new Date(2026, 9, 21, 23).getTime()).queryKey;
    const next = projectsQuery(new Date(2026, 9, 22, 0, 1).getTime()).queryKey;
    expect(evening).toEqual(morning);
    expect(next).not.toEqual(morning);
    expect(next.slice(0, 1)).toEqual(keys.projects());
  });

  it("keeps host keys out of every slug's prefix", () => {
    for (const key of [keys.projects(), keys.session()]) {
      expect(parseSlug(key[0])).toBeNull();
    }
  });
});
