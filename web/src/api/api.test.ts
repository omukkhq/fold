import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isApiError } from "./errors";
import { FIXTURE_DELAY_MS, fromFixture } from "./fixture";
import { keys } from "./keys";
import { getProject, listProjects } from "./projects";
import { projectQuery, projectsQuery, sessionQuery } from "./queries";
import { parseSlug } from "../routes/params";
import { getSession } from "./session";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("fromFixture", () => {
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

  it("answers with empty values and reads no fixture", async () => {
    vi.stubEnv("DEV", false);
    vi.resetModules();
    const projects = await import("./projects");
    const session = await import("./session");
    const errors = await import("./errors");
    await expect(projects.listProjects()).resolves.toEqual([]);
    await expect(session.getSession()).resolves.toEqual({
      humanName: null,
      authenticated: true,
    });
    await expect(projects.getProject("demo")).rejects.toSatisfy((e: unknown) =>
      errors.isApiError(e, "project_not_found"),
    );
  });
});

describe("projects", () => {
  it("lists the fixture projects", async () => {
    const list = listProjects();
    await vi.runAllTimersAsync();
    expect((await list).map((p) => p.slug)).toContain("demo");
  });

  it("finds a project by slug", async () => {
    const found = getProject("demo");
    await vi.runAllTimersAsync();
    expect((await found).slug).toBe("demo");
  });

  it("rejects an unknown slug with the project_not_found code", async () => {
    const missing = getProject("nope");
    const check = expect(missing).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "project_not_found"),
    );
    await vi.runAllTimersAsync();
    await check;
  });

  it("rejects a project that failed to open with the project_unavailable code", async () => {
    const closed = getProject("archive");
    const check = expect(closed).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "project_unavailable"),
    );
    await vi.runAllTimersAsync();
    await check;
  });
});

describe("session", () => {
  it("returns the human name", async () => {
    const s = getSession();
    await vi.runAllTimersAsync();
    expect((await s).humanName).not.toBe("");
  });
});

describe("queries", () => {
  it("fetch through the api functions under the key builders", async () => {
    const client = new QueryClient();
    const all = Promise.all([
      client.query(projectsQuery()),
      client.query(projectQuery("demo")),
      client.query(sessionQuery()),
    ]);
    await vi.runAllTimersAsync();
    const [list, project, who] = await all;
    expect(list.length).toBeGreaterThan(0);
    expect(project.slug).toBe("demo");
    expect(who.humanName).not.toBe("");
    expect(client.getQueryData(keys.project("demo"))).toEqual(project);
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
