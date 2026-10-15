import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseSlug } from "../routes/params";
import { stubFetch } from "../test/stubFetch";
import { RECENT_EVENTS } from "./agents";
import { agentBarQuery } from "./agentBar";
import { FIXTURE_DELAY_MS, fromFixture } from "./fixture";
import { keys } from "./keys";
import {
  agentsQuery,
  awaitingHumanQuery,
  messagesQuery,
  projectQuery,
  projectsQuery,
  sessionQuery,
} from "./queries";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("fromFixture", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  it("resolves a copy of the fixture only after the delay", async () => {
    const value = { a: 1 };
    const pending = fromFixture(
      () => Promise.resolve({ value }),
      (m) => m.value,
      { a: 0 },
    );
    let settled = false;
    void pending.then(() => {
      settled = true;
    });
    await vi.advanceTimersByTimeAsync(FIXTURE_DELAY_MS - 1);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    const result = await pending;
    expect(result).toEqual(value);
    expect(result).not.toBe(value);
  });

  it("answers with the empty value at once when there is no fixture", async () => {
    await expect(fromFixture(undefined, () => 1, 0)).resolves.toBe(0);
  });
});

describe("a production build", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("lists no event or review checkout and reads no fixture", async () => {
    vi.stubEnv("DEV", false);
    vi.resetModules();
    const events = await import("./events");
    const reviews = await import("./reviews");
    await expect(events.listEvents("demo")).resolves.toEqual([]);
    await expect(reviews.listReviewCheckouts("demo")).resolves.toEqual([]);
  });
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

describe("awaitingHumanQuery", () => {
  it("sits under the messages key, so message events refresh it", async () => {
    const query = awaitingHumanQuery("demo");
    expect(query.queryKey.slice(0, 2)).toEqual(keys.messages("demo"));
    stubFetch({
      "GET /api/v1/projects/demo/messages": {
        body: { items: [], next: null, total_awaiting_human: 2 },
      },
    });
    await expect(new QueryClient().query(query)).resolves.toBe(2);
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
      keys.messagesAwaiting("s"),
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

  it("keeps host keys out of every slug's prefix", () => {
    for (const key of [keys.projects(), keys.session()]) {
      expect(parseSlug(key[0])).toBeNull();
    }
  });
});
