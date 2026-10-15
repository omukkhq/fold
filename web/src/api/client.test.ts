import createClient from "openapi-fetch";
import { describe, expect, it, vi } from "vitest";
import { api, authRequiredMiddleware } from "./client";
import type { paths } from "./schema";
import { apiErrorCode } from "./errors";
import { expectOk, unwrap } from "./result";

function respond(status: number, body: unknown) {
  const requests: Request[] = [];
  const fetch = (request: Request) => {
    requests.push(request);
    return Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
    );
  };
  return { requests, fetch };
}

describe("api", () => {
  it("requests the declared path and returns the typed body", async () => {
    const stub = respond(200, {
      projects: [{ slug: "fold", name: "fold", available: true }],
    });
    const { data, error } = await api.GET("/api/v1/projects", {
      baseUrl: "http://fold.test/",
      fetch: stub.fetch,
    });
    expect(error).toBeUndefined();
    expect(data?.projects[0]?.slug).toBe("fold");
    expect(stub.requests[0]?.url).toBe("http://fold.test/api/v1/projects");
  });

  it("returns the error envelope of a failed request", async () => {
    const stub = respond(404, {
      error: { code: "project_not_found", message: "missing", details: {} },
    });
    const { data, error } = await api.GET("/api/v1/projects/{slug}", {
      baseUrl: "http://fold.test/",
      fetch: stub.fetch,
      params: { path: { slug: "ghost" } },
    });
    expect(data).toBeUndefined();
    expect(apiErrorCode(error)).toBe("project_not_found");
    expect(stub.requests[0]?.url).toBe(
      "http://fold.test/api/v1/projects/ghost",
    );
  });
});

describe("unwrap and expectOk through the client", () => {
  const baseUrl = "http://fold.test/";

  it("throws the ApiError of an error envelope", async () => {
    const stub = respond(404, {
      error: { code: "project_not_found", message: "missing", details: {} },
    });
    await expect(
      unwrap(
        api.GET("/api/v1/projects/{slug}", {
          baseUrl,
          fetch: stub.fetch,
          params: { path: { slug: "ghost" } },
        }),
      ),
    ).rejects.toMatchObject({ code: "project_not_found" });
  });

  it("throws daemon_unavailable for a failure without the envelope", async () => {
    const stub = respond(502, "bad gateway");
    await expect(
      expectOk(api.GET("/api/v1/projects", { baseUrl, fetch: stub.fetch })),
    ).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "daemon_unavailable",
    );
  });

  it("returns the body of a success", async () => {
    const stub = respond(200, { projects: [] });
    await expect(
      unwrap(api.GET("/api/v1/projects", { baseUrl, fetch: stub.fetch })),
    ).resolves.toEqual({ projects: [] });
  });
});

describe("authRequiredMiddleware", () => {
  const baseUrl = "http://fold.test/";

  async function ask(status: number, body: unknown) {
    const onAuthRequired = vi.fn();
    const client = createClient<paths>({ baseUrl });
    client.use(authRequiredMiddleware(onAuthRequired));
    const stub = respond(status, body);
    const answer = await client.GET("/api/v1/projects", { fetch: stub.fetch });
    return { onAuthRequired, answer };
  }

  it("fires for auth_required and leaves the answer readable", async () => {
    const { onAuthRequired, answer } = await ask(401, {
      error: { code: "auth_required", message: "", details: {} },
    });
    expect(onAuthRequired).toHaveBeenCalledOnce();
    expect(apiErrorCode(answer.error)).toBe("auth_required");
  });

  it.each([
    [401, { error: { code: "invalid_token", message: "", details: {} } }],
    [401, "not json"],
    [200, { projects: [] }],
  ])("ignores status %i with body %j", async (status, body) => {
    const { onAuthRequired } = await ask(status, body);
    expect(onAuthRequired).not.toHaveBeenCalled();
  });
});
