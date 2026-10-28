import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { notifySend, notifyTarget } from "../test/notify";
import { isApiError } from "./errors";
import {
  addNotifyTarget,
  editNotifyTarget,
  getProjectNotify,
  listNotifySends,
  removeNotifyTarget,
  rotateNotifyTarget,
  sendNotifyAgain,
  setUseHostTargets,
  setWebUrl,
  testNotifyTarget,
} from "./notify";

afterEach(() => {
  vi.unstubAllGlobals();
});

const project = { kind: "project", slug: "demo" } as const;
const host = { kind: "host" } as const;
const added = { target: notifyTarget(), secret: "whsec_AAAA" };
const settings = {
  targets: [notifyTarget()],
  host_targets: [],
  use_host_targets: true,
  web_url: null,
};

describe("the notice settings", () => {
  it("read and write the project's and the host's settings", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/notify": { body: settings },
      "PUT /api/v1/notify": { body: { targets: [], web_url: "x" } },
      "PUT /api/v1/projects/demo/notify": { body: settings },
    });
    expect(await getProjectNotify("demo")).toEqual(settings);
    expect((await setWebUrl("x")).web_url).toBe("x");
    await setUseHostTargets("demo", false);
    expect(calls.map((call) => call.body)).toEqual([
      undefined,
      { web_url: "x" },
      { use_host_targets: false },
    ]);
  });

  it("add, change, rotate and remove targets of the project and of the host", async () => {
    const calls = stubFetch({
      "POST /api/v1/projects/demo/notify/targets": { status: 201, body: added },
      "POST /api/v1/notify/targets": { status: 201, body: added },
      "PUT /api/v1/projects/demo/notify/targets/nt-1": { body: notifyTarget() },
      "PUT /api/v1/notify/targets/nt-1": { body: notifyTarget() },
      "POST /api/v1/projects/demo/notify/targets/nt-1/rotate": { body: added },
      "POST /api/v1/notify/targets/nt-1/rotate": { body: added },
      "DELETE /api/v1/projects/demo/notify/targets/nt-1": { status: 204 },
      "DELETE /api/v1/notify/targets/nt-1": { status: 204 },
    });
    const body = { kind: "slack", name: "Team Slack", url: "u" } as const;
    expect((await addNotifyTarget(project, body)).secret).toBe("whsec_AAAA");
    await addNotifyTarget(host, body);
    await editNotifyTarget(project, "nt-1", { name: "x" });
    await editNotifyTarget(host, "nt-1", { enabled: false });
    await rotateNotifyTarget(project, "nt-1");
    await rotateNotifyTarget(host, "nt-1");
    await removeNotifyTarget(project, "nt-1");
    await removeNotifyTarget(host, "nt-1");
    expect(calls.map((call) => `${call.method} ${call.path}`)).toHaveLength(8);
  });

  it("test a target, read its sends and send one again", async () => {
    stubFetch({
      "POST /api/v1/projects/demo/notify/targets/nt-1/test": {
        body: notifySend({ test: true }),
      },
      "GET /api/v1/projects/demo/notify/targets/nt-1/sends": {
        body: { sends: [notifySend()] },
      },
      "POST /api/v1/projects/demo/notify/sends/ns-1/again": {
        body: notifySend({ state: "pending", tries: 0 }),
      },
      "POST /api/v1/projects/demo/notify/targets/nt-2/test": errorReply(
        502,
        "notify_send_failed",
      ),
    });
    expect((await testNotifyTarget("demo", "nt-1")).test).toBe(true);
    expect(await listNotifySends("demo", "nt-1")).toEqual([notifySend()]);
    expect((await sendNotifyAgain("demo", "ns-1")).state).toBe("pending");
    await expect(testNotifyTarget("demo", "nt-2")).rejects.toSatisfy(
      (error: unknown) => isApiError(error, "notify_send_failed"),
    );
  });
});
