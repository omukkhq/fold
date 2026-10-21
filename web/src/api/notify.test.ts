import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import {
  addNotifyTarget,
  getHostNotify,
  getProjectNotify,
  removeNotifyTarget,
  setWebUrl,
  testNotifyTarget,
  type NotifyScope,
} from "./notify";

afterEach(() => {
  vi.unstubAllGlobals();
});

const HOST: NotifyScope = { kind: "host" };
const DEMO: NotifyScope = { kind: "project", slug: "demo" };
const settings = {
  targets: [],
  host_targets: [],
  uses_host: true,
  link_base: "local",
};

describe("notification settings", () => {
  it("reads the project's and the host's settings", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/notify": { body: settings },
      "GET /api/v1/notify": {
        body: { targets: [], web_url: null, link_base: "local" },
      },
    });
    expect(await getProjectNotify("demo")).toEqual(settings);
    expect((await getHostNotify()).web_url).toBeNull();
  });

  it("sends each write to the route of its scope", async () => {
    const entry = { id: "nt-1" };
    const calls = stubFetch({
      "PUT /api/v1/notify": {
        body: { targets: [], web_url: "x", link_base: "x" },
      },
      "POST /api/v1/notify/targets": { status: 201, body: entry },
      "POST /api/v1/projects/demo/notify/targets": { status: 201, body: entry },
      "DELETE /api/v1/notify/targets/nt-1": { status: 204 },
      "DELETE /api/v1/projects/demo/notify/targets/nt-1": { status: 204 },
      "POST /api/v1/notify/targets/nt-1/test": { status: 204 },
      "POST /api/v1/projects/demo/notify/targets/nt-1/test": { status: 204 },
    });
    await setWebUrl("x");
    const body = { kind: "webhook" as const, url: "u", secret: "s" };
    for (const scope of [HOST, DEMO]) {
      expect(await addNotifyTarget(scope, body)).toEqual(entry);
      await removeNotifyTarget(scope, "nt-1");
      await testNotifyTarget(scope, "nt-1");
    }
    expect(calls.map((call) => `${call.method} ${call.path}`)).toEqual([
      "PUT /api/v1/notify",
      "POST /api/v1/notify/targets",
      "DELETE /api/v1/notify/targets/nt-1",
      "POST /api/v1/notify/targets/nt-1/test",
      "POST /api/v1/projects/demo/notify/targets",
      "DELETE /api/v1/projects/demo/notify/targets/nt-1",
      "POST /api/v1/projects/demo/notify/targets/nt-1/test",
    ]);
    expect(calls[0]?.body).toEqual({ web_url: "x" });
    expect(calls[1]?.body).toEqual(body);
  });

  it("rejects a failed test with the daemon's code", async () => {
    stubFetch({
      "POST /api/v1/notify/targets/nt-2/test": errorReply(
        502,
        "notify_send_failed",
      ),
    });
    await expect(testNotifyTarget(HOST, "nt-2")).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "notify_send_failed"),
    );
  });
});
