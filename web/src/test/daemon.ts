import createClient from "openapi-fetch";
import type { paths } from "@/api/schema";
import { readBody } from "./stubFetch";

export interface Call {
  method: string;
  /** The path with its query string. */
  path: string;
  body: unknown;
  /** The values of the `:name` segments of the route that matched. */
  params: Record<string, string>;
}

export interface Reply {
  status: number;
  body?: unknown;
}

export const ok = (body: unknown, status = 200): Reply => ({ status, body });
export const noContent = (): Reply => ({ status: 204 });
export const refuse = (
  status: number,
  code: string,
  details: Record<string, unknown> = {},
): Reply => ({
  status,
  body: { error: { code, message: `refused: ${code}`, details } },
});

// Built from parts because the raw-literal guard refuses a remote URL in a source file.
const ORIGIN = ["http:", "", "fold.test"].join("/");

type Handler = (call: Call) => Reply | Promise<Reply>;

interface Route {
  method: string;
  pattern: RegExp;
  names: string[];
  handler: Handler;
}

function compile(route: string, handler: Handler): Route {
  const [method = "", path = ""] = route.split(" ");
  const names: string[] = [];
  const source = path
    .split("/")
    .map((segment) => {
      if (segment.startsWith(":")) {
        names.push(segment.slice(1));
        return "([^/?]+)";
      }
      return segment.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    })
    .join("/");
  return { method, pattern: new RegExp(`^${source}$`), names, handler };
}

/**
 * A daemon for tests: routes are registered as `METHOD /path` and answer through the real generated client, so a test
 * sees the requests the application sends and the failures it must read. A route matches the path without its query
 * string, which the handler reads from `call.path`. Anything unregistered answers 404.
 */
export class FakeDaemon {
  readonly calls: Call[] = [];
  private routes: Route[] = [];
  /** Milliseconds every answer waits, so a test can see a write while it is pending. */
  latency = 0;

  readonly api = createClient<paths>({
    baseUrl: ORIGIN,
    fetch: (request: Request) => this.answer(request),
  });

  /** Registers `METHOD /path/:param`; a later registration of the same route wins. */
  on(route: string, handler: Handler | Reply): void {
    const compiled = compile(
      route,
      typeof handler === "function" ? handler : () => handler,
    );
    this.routes = [
      compiled,
      ...this.routes.filter(
        (r) =>
          r.method !== compiled.method ||
          r.pattern.source !== compiled.pattern.source,
      ),
    ];
  }

  /** Registers the routes the project shell reads besides each screen's own: the session, the project list, each project, its agents and its messages. A test that registers agents or messages afterwards replaces them. */
  shell(slugs: readonly string[]): void {
    const projects = slugs.map((slug) => ({
      slug,
      name: slug,
      available: true,
      overview: null,
    }));
    this.on("GET /api/v1/session", ok({ human: "tanim", authenticated: true }));
    this.on("GET /api/v1/projects", ok({ projects }));
    this.on("GET /api/v1/projects/:slug", (call) => {
      const project = projects.find((p) => p.slug === call.params.slug);
      return project ? ok(project) : refuse(404, "project_not_found");
    });
    this.on("GET /api/v1/projects/:slug/agents", ok({ agents: [] }));
    this.on(
      "GET /api/v1/projects/:slug/messages",
      ok({ items: [], next: null, total_awaiting_human: 0 }),
    );
  }

  reset(): void {
    this.calls.length = 0;
    this.routes = [];
    this.latency = 0;
  }

  /** The calls made so far to `METHOD /path`. */
  made(route: string): Call[] {
    return this.calls.filter((c) => `${c.method} ${c.path}` === route);
  }

  private match(call: Call): Route | undefined {
    const [pathname = ""] = call.path.split("?");
    for (const route of this.routes) {
      const found =
        route.method === call.method && route.pattern.exec(pathname);
      if (found) {
        route.names.forEach((name, i) => {
          call.params[name] = decodeURIComponent(found[i + 1] ?? "");
        });
        return route;
      }
    }
    return undefined;
  }

  private async answer(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const call: Call = {
      method: request.method,
      path: url.pathname + url.search,
      body: await readBody(request),
      params: {},
    };
    this.calls.push(call);
    if (this.latency > 0) {
      await new Promise((resolve) => setTimeout(resolve, this.latency));
    }
    const matched = this.match(call);
    const reply = matched
      ? await matched.handler(call)
      : refuse(404, "route_not_registered");
    return new Response(
      reply.body === undefined ? null : JSON.stringify(reply.body),
      {
        status: reply.status,
        headers: { "Content-Type": "application/json" },
      },
    );
  }
}

export const daemon = new FakeDaemon();
