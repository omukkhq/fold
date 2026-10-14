import { describe, expect, it } from "vitest";
import { api } from "./client";
import { apiErrorCode } from "./errors";

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
