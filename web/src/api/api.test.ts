import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isApiError } from "./errors";
import { FIXTURE_DELAY_MS, fromFixture } from "./fixture";
import { keys } from "./keys";
import { getProject, listProjects } from "./projects";
import { listAgents, setIntegrator } from "./agents";
import { listThreads, sendMessage } from "./messages";
import {
  agentsQuery,
  messagesQuery,
  projectQuery,
  projectsQuery,
  sessionQuery,
} from "./queries";
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

  it("lists no agent or thread and refuses writes with the daemon's not_found", async () => {
    vi.stubEnv("DEV", false);
    vi.resetModules();
    const agents = await import("./agents");
    const messages = await import("./messages");
    const errors = await import("./errors");
    await expect(agents.listAgents()).resolves.toEqual([]);
    await expect(messages.listThreads()).resolves.toEqual([]);
    await expect(agents.setIntegrator("review", true)).rejects.toSatisfy(
      (e: unknown) => errors.isApiError(e, "not_found"),
    );
    await expect(
      messages.sendMessage({ task: "T-12", body: "hi" }),
    ).rejects.toSatisfy((e: unknown) => errors.isApiError(e, "not_found"));
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

async function settle<T>(pending: Promise<T>): Promise<T> {
  const settled = pending.then(
    (value) => ({ ok: true as const, value }),
    (error: unknown) => ({ ok: false as const, error }),
  );
  await vi.runAllTimersAsync();
  const result = await settled;
  if (!result.ok) {
    throw result.error;
  }
  return result.value;
}

describe("agents", () => {
  it("lists the fixture agents", async () => {
    const list = await settle(listAgents());
    expect(list.map((a) => a.name)).toEqual(["frontend", "backend", "review"]);
  });

  it("grants and removes integrator through the fixture", async () => {
    await settle(setIntegrator("frontend", true));
    let frontend = (await settle(listAgents())).find(
      (a) => a.name === "frontend",
    );
    expect(frontend?.tags).toContain("integrator");
    expect(frontend?.integrator_granted_by).not.toBeNull();
    await settle(setIntegrator("frontend", false));
    frontend = (await settle(listAgents())).find((a) => a.name === "frontend");
    expect(frontend?.tags).not.toContain("integrator");
    expect(frontend?.integrator_granted_by).toBeNull();
  });

  it("rejects an unknown agent with agent_not_found", async () => {
    // The production test above resets the module registry, so the fixture throws a copy of ApiError: compare the code, not the class.
    await expect(settle(setIntegrator("nobody", true))).rejects.toMatchObject({
      code: "agent_not_found",
    });
  });
});

describe("messages", () => {
  it("adds a reply to its thread and reports the answered question", async () => {
    const threads = await settle(listThreads());
    const open = threads.find((t) => t.unread);
    const last = open?.messages.at(-1);
    expect(last).toBeDefined();
    const sent = await settle(
      sendMessage({ reply_to: last?.id ?? "", body: "Plain SVG." }),
    );
    expect(sent.message.thread).toBe(open?.id);
    expect(sent.answered).toEqual([last?.id]);
    const after = (await settle(listThreads())).find((t) => t.id === open?.id);
    expect(after?.unread).toBe(false);
    expect(after?.messages.at(-1)?.body).toBe("Plain SVG.");
  });

  it("starts a thread for feedback on a task", async () => {
    const sent = await settle(sendMessage({ task: "T-14", body: "Check it." }));
    expect(sent.answered).toEqual([]);
    const thread = (await settle(listThreads())).find(
      (t) => t.id === sent.message.thread,
    );
    expect(thread?.task).toBe("T-14");
    expect(thread?.participants).toContain("backend/api");
  });

  it("rejects a reply to an unknown message with message_not_found", async () => {
    await expect(
      settle(sendMessage({ reply_to: "m-9999", body: "x" })),
    ).rejects.toMatchObject({ code: "message_not_found" });
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
      client.query(agentsQuery("demo")),
      client.query(messagesQuery("demo")),
    ]);
    await vi.runAllTimersAsync();
    const [list, project, who, agents, threads] = await all;
    expect(agents.length).toBeGreaterThan(0);
    expect(threads.length).toBeGreaterThan(0);
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
