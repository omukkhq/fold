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

/** A multipart form as a test reads it: the text parts by name and the file parts with their file names, in order. */
export interface MultipartBody {
  fields: Record<string, string>;
  files: { part: string; name: string; type: string }[];
}

/**
 * Splits a multipart body by hand. The runtime's own parser rejects the bytes under jsdom, whose typed arrays come from another realm, and jsdom's encoder drops file bytes, so only names and types are read.
 */
function parseMultipart(text: string, boundary: string): MultipartBody {
  const body: MultipartBody = { fields: {}, files: [] };
  for (const raw of text.split(`--${boundary}`).slice(1, -1)) {
    const [head = "", ...rest] = raw.replace(/^\r\n/, "").split("\r\n\r\n");
    const value = rest.join("\r\n\r\n").replace(/\r\n$/, "");
    const part = /name="([^"]*)"/.exec(head)?.[1] ?? "";
    const file = /filename="([^"]*)"/.exec(head)?.[1];
    if (file === undefined) {
      body.fields[part] = value;
    } else {
      const type = /Content-Type: (.+)/i.exec(head)?.[1]?.trim() ?? "";
      body.files.push({ part, name: file, type });
    }
  }
  return body;
}

/** A multipart body arrives as a `MultipartBody`, any other body as parsed JSON. */
export async function readBody(input: Request): Promise<unknown> {
  const type = input.headers.get("Content-Type") ?? "";
  const text = await input.text();
  const boundary = /boundary=(.+)$/.exec(type)?.[1];
  if (type.startsWith("multipart/form-data") && boundary) {
    return parseMultipart(text, boundary);
  }
  return text === "" ? undefined : (JSON.parse(text) as unknown);
}

/** Replaces `fetch` so that `METHOD /path` answers with the given reply; a request no route names answers 404 `not_found`. Undo with `vi.unstubAllGlobals()`. */
export function stubFetch(routes: Record<string, Route>): StubRequest[] {
  const calls: StubRequest[] = [];
  vi.stubGlobal("fetch", async (input: Request) => {
    const url = new URL(input.url);
    const request: StubRequest = {
      method: input.method,
      path: url.pathname,
      search: url.searchParams,
      body: await readBody(input),
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
 * The routes every screen inside the project shell reads besides its own: the session, the project list, each project, its agents, its messages and what needs a human.
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
    routes[`GET ${base}/needs-you`] = { body: { items: [] } };
  }
  return routes;
}
