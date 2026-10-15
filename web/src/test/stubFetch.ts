import { vi } from "vitest";

export interface StubRequest {
  method: string;
  path: string;
  search: URLSearchParams;
  body: unknown;
}

export interface StubReply {
  status?: number;
  body?: unknown;
}

type Route =
  | StubReply
  | ((request: StubRequest) => StubReply | Promise<StubReply>);

/** Replaces `fetch` so that `METHOD /path` answers with the given reply; a request no route names answers 404 `not_found`. Undo with `vi.unstubAllGlobals()`. */
export function stubFetch(routes: Record<string, Route>): StubRequest[] {
  const calls: StubRequest[] = [];
  vi.stubGlobal("fetch", async (input: Request) => {
    const url = new URL(input.url);
    const text = await input.text();
    const request: StubRequest = {
      method: input.method,
      path: url.pathname,
      search: url.searchParams,
      body: text === "" ? undefined : (JSON.parse(text) as unknown),
    };
    calls.push(request);
    const route = routes[`${request.method} ${request.path}`];
    const reply: StubReply =
      typeof route === "function"
        ? await route(request)
        : (route ?? {
            status: 404,
            body: { error: { code: "not_found", message: "", details: {} } },
          });
    const status = reply.status ?? 200;
    return new Response(
      reply.body === undefined || status === 204
        ? null
        : JSON.stringify(reply.body),
      { status, headers: { "Content-Type": "application/json" } },
    );
  });
  return calls;
}

/** The envelope of an error answer. */
export function errorReply(status: number, code: string): StubReply {
  return { status, body: { error: { code, message: "", details: {} } } };
}

/**
 * The routes every screen inside the project shell reads besides its own: the session, the project list, each project, its agents and its messages.
 */
export function shellRoutes(slugs: readonly string[]): Record<string, Route> {
  const projects = slugs.map((slug) => ({ slug, name: slug, available: true }));
  const routes: Record<string, Route> = {
    "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
    "GET /api/v1/projects": { body: { projects } },
  };
  for (const project of projects) {
    const base = `/api/v1/projects/${project.slug}`;
    routes[`GET ${base}`] = { body: project };
    routes[`GET ${base}/agents`] = { body: { agents: [] } };
    routes[`GET ${base}/messages`] = {
      body: { items: [], next: null, total_awaiting_human: 0 },
    };
  }
  return routes;
}
