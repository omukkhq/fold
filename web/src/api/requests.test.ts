import { afterEach, describe, expect, it, vi } from "vitest";
import {
  requestDetail,
  requestMessage,
  requestSummary,
} from "../test/requests";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { keys } from "./keys";
import { requestQuery, requestThreadQuery, requestsQuery } from "./queries";
import {
  REQUEST_TOO_LARGE,
  approveRequestDraft,
  attachmentHref,
  closeRequest,
  createRequest,
  editRequestDraft,
  getRequest,
  getThread,
  listRequests,
  requestDraftChanges,
  sendRequestMessage,
} from "./requests";

afterEach(() => {
  vi.unstubAllGlobals();
});

const R = "/api/v1/projects/demo/requests";
const detail = requestDetail(requestSummary("RQ-4"));

describe("request reads", () => {
  it("lists the requests and reads one, null for one the project does not have", async () => {
    stubFetch({
      [`GET ${R}`]: {
        body: { items: [requestSummary("RQ-2"), requestSummary("RQ-1")] },
      },
      [`GET ${R}/RQ-4`]: { body: detail },
      [`GET ${R}/RQ-9`]: errorReply(404, "request_not_found"),
      [`GET ${R}/RQ-5`]: errorReply(503, "project_unavailable"),
    });
    expect((await listRequests("demo")).map((r) => r.id)).toEqual([
      "RQ-2",
      "RQ-1",
    ]);
    await expect(getRequest("demo", "RQ-4")).resolves.toEqual(detail);
    await expect(getRequest("demo", "RQ-9")).resolves.toBeNull();
    await expect(getRequest("demo", "RQ-5")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "project_unavailable"),
    );
  });

  it("reads the clarification thread by its id", async () => {
    const thread = {
      id: "m-1",
      awaiting_human: false,
      last_at: 1,
      messages: [requestMessage(1, "backend", "human:tanim", "Archived too?")],
      participants: ["backend", "human:tanim"],
      proposal: null,
      request: "RQ-4",
      task: null,
    };
    stubFetch({ "GET /api/v1/projects/demo/threads/m-1": { body: thread } });
    await expect(getThread("demo", "m-1")).resolves.toEqual(thread);
  });

  it("keys the queries under the project, the thread under its request", () => {
    expect(requestsQuery("demo").queryKey).toEqual(keys.requests("demo"));
    expect(requestQuery("demo", "RQ-4").queryKey).toEqual([
      "demo",
      "request",
      "RQ-4",
    ]);
    expect(
      requestThreadQuery("demo", "RQ-4", "m-1").queryKey.slice(0, 3),
    ).toEqual(keys.request("demo", "RQ-4"));
  });
});

describe("creating a request", () => {
  it("sends the message and every file as one multipart form", async () => {
    const calls = stubFetch({ [`POST ${R}`]: { status: 201, body: detail } });
    const files = [
      new File(["a,b"], "sample.csv", { type: "text/csv" }),
      new File(["png"], "mockup.png", { type: "image/png" }),
    ];
    await expect(createRequest("demo", "Need #T-9", files)).resolves.toEqual(
      detail,
    );
    expect(calls[0]?.body).toEqual({
      fields: { body: "Need #T-9" },
      files: [
        { part: "files", name: "sample.csv", type: "text/csv" },
        { part: "files", name: "mockup.png", type: "image/png" },
      ],
    });
  });

  it("gives the plain 413 answer its own code", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.resolve(new Response("length limit exceeded", { status: 413 })),
    );
    await expect(createRequest("demo", "x", [])).rejects.toSatisfy(
      (e: unknown) => isApiError(e, REQUEST_TOO_LARGE),
    );
  });

  it("passes a refusal on with its code", async () => {
    stubFetch({ [`POST ${R}`]: errorReply(401, "human_name_required") });
    await expect(createRequest("demo", "x", [])).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "human_name_required"),
    );
  });
});

describe("requester writes", () => {
  it("posts each action with the version it read", async () => {
    const calls = stubFetch({
      [`POST ${R}/RQ-4/changes`]: { body: detail },
      [`POST ${R}/RQ-4/drafts`]: { body: detail },
      [`POST ${R}/RQ-4/approve`]: { body: detail },
      [`POST ${R}/RQ-4/close`]: { body: detail },
    });
    await requestDraftChanges("demo", "RQ-4", 2, "split it");
    await editRequestDraft("demo", "RQ-4", { cards: [], edges: [] });
    await approveRequestDraft("demo", "RQ-4", 3);
    await closeRequest("demo", "RQ-4", "done in #T-9");
    expect(calls.map((c) => [c.path, c.body])).toEqual([
      [`${R}/RQ-4/changes`, { version: 2, comment: "split it" }],
      [`${R}/RQ-4/drafts`, { cards: [], edges: [] }],
      [`${R}/RQ-4/approve`, { expected_version: 3 }],
      [`${R}/RQ-4/close`, { reason: "done in #T-9" }],
    ]);
  });

  it("sends a thread message with the request and, when given, its recipients", async () => {
    const message = requestMessage(3, "human:tanim", "backend", "No.");
    const calls = stubFetch({
      "POST /api/v1/projects/demo/feedback": { body: message },
    });
    await sendRequestMessage("demo", "RQ-4", "No.");
    await sendRequestMessage("demo", "RQ-4", "Hi", ["all"]);
    expect(calls.map((c) => c.body)).toEqual([
      { body: "No.", request: "RQ-4" },
      { body: "Hi", request: "RQ-4", to: ["all"] },
    ]);
  });

  it("builds the download address of an attachment", () => {
    expect(attachmentHref("demo", "RQ-4", 2)).toBe(
      "/api/v1/projects/demo/requests/RQ-4/attachments/2",
    );
  });
});
