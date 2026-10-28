// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { ProjectNotify } from "@/api/notify";
import { notifySend, notifyTarget } from "@/test/notify";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { SettingsScreen } from "./SettingsScreen";

beforeAll(() => {
  // Radix sizes the checkbox's hidden form input with a ResizeObserver, which jsdom lacks.
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const SECURE = ["https:", "", "ci.example.org", "fold-hook"].join("/");

function settings(fields: Partial<ProjectNotify> = {}): ProjectNotify {
  return {
    targets: [notifyTarget()],
    host_targets: [
      notifyTarget({
        id: "nt-2",
        name: "Release bot",
        kind: "webhook",
        host: true,
      }),
    ],
    use_host_targets: true,
    web_url: null,
    ...fields,
  };
}

function show() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <SettingsScreen slug="demo" />
    </QueryClientProvider>,
  );
}

describe("SettingsScreen", () => {
  it("lists the project's and the host's targets with the host switch and the link hint", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/notify": { body: settings() },
    });
    show();
    const list = await screen.findByRole("list", { name: "Targets" });
    expect(within(list).getByText("Team Slack")).toBeTruthy();
    expect(within(list).getByText("Release bot")).toBeTruthy();
    expect(within(list).getByText("Host target")).toBeTruthy();
    expect(
      screen.getByText("2 targets get this project's notices."),
    ).toBeTruthy();
    expect(screen.getByText(/notices carry no links/)).toBeTruthy();
    expect(
      screen
        .getByRole("checkbox", { name: /Use the host targets/ })
        .getAttribute("aria-checked"),
    ).toBe("true");
  });

  it("adds a generic target and shows its secret once", async () => {
    let added = false;
    const calls = stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/notify": () => ({
        body: settings({
          targets: added
            ? [notifyTarget({ id: "nt-3", name: "CI", kind: "webhook" })]
            : [],
        }),
      }),
      "POST /api/v1/projects/demo/notify/targets": () => {
        added = true;
        return {
          status: 201,
          body: {
            target: notifyTarget({ id: "nt-3", name: "CI", kind: "webhook" }),
            secret: "whsec_SECRET",
          },
        };
      },
    });
    show();
    const form = await screen.findByRole("form", { name: "Add a target" });
    await userEvent.type(within(form).getByLabelText("Name"), "CI");
    await userEvent.click(
      within(form).getByRole("button", { name: "Generic JSON" }),
    );
    await userEvent.type(within(form).getByLabelText(/Webhook URL/), SECURE);
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));
    expect(await screen.findByText("whsec_SECRET")).toBeTruthy();
    const post = calls.find((call) => call.method === "POST");
    expect(post?.body).toEqual({
      kind: "webhook",
      name: "CI",
      url: SECURE,
      preset: "recommended",
      moves_to: ["review", "done"],
      window_seconds: 15,
    });
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByText("whsec_SECRET")).toBeNull();
  });

  it("checks the form before it sends anything", async () => {
    const calls = stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/notify": { body: settings() },
    });
    show();
    const form = await screen.findByRole("form", { name: "Add a target" });
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));
    expect(
      await within(form).findByText("Give a name of 1 to 64 characters."),
    ).toBeTruthy();
    expect(calls.some((call) => call.method === "POST")).toBe(false);
  });

  it("sends a test, shows a failed one and opens the send log with Send again", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/notify": { body: settings() },
      "POST /api/v1/projects/demo/notify/targets/nt-1/test": {
        body: notifySend({ test: true }),
      },
      "POST /api/v1/projects/demo/notify/targets/nt-2/test": errorReply(
        502,
        "notify_send_failed",
      ),
      "GET /api/v1/projects/demo/notify/targets/nt-1/sends": {
        body: {
          sends: [notifySend({ state: "given_up", status: 503, tries: 7 })],
        },
      },
      "POST /api/v1/projects/demo/notify/sends/ns-1/again": {
        body: notifySend({ state: "pending" }),
      },
    });
    show();
    await userEvent.click(
      await screen.findByRole("button", {
        name: "Send a test through Team Slack",
      }),
    );
    expect(await screen.findByText("The receiver took the test.")).toBeTruthy();
    await userEvent.click(
      screen.getByRole("button", { name: "Send a test through Release bot" }),
    );
    expect(await screen.findByText("The test send failed.")).toBeTruthy();
    await userEvent.click(
      screen.getByRole("button", { name: "Sends of Team Slack" }),
    );
    const log = await screen.findByRole("region", {
      name: "Recent sends of Team Slack",
    });
    expect(within(log).getByText("503 · try 7")).toBeTruthy();
    await userEvent.click(
      within(log).getByRole("button", { name: "Send ns-1 again" }),
    );
  });

  it("changes nothing for a viewer who is not signed in as a person", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: null, authenticated: true } },
      "GET /api/v1/projects/demo/notify": {
        body: settings({ use_host_targets: false }),
      },
    });
    show();
    expect(
      await screen.findByText(/only a person changes where notices go/),
    ).toBeTruthy();
    const list = await screen.findByRole("list", { name: "Targets" });
    const host = within(list).getByRole("listitem", { name: "Release bot" });
    expect(
      within(host).getByText(
        "Host target · this project does not use the host targets",
      ),
    ).toBeTruthy();
    expect(
      screen
        .getByRole<HTMLButtonElement>("button", {
          name: "Send a test through Team Slack",
        })
        .hasAttribute("disabled"),
    ).toBe(true);
  });

  it("removes a target only once the person confirms, and stays pending until the daemon answers", async () => {
    let removed = false;
    let answer: (() => void) | undefined;
    const calls = stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/notify": () => ({
        body: settings({ targets: removed ? [] : [notifyTarget()] }),
      }),
      "DELETE /api/v1/projects/demo/notify/targets/nt-1": () =>
        new Promise((resolve) => {
          answer = () => {
            removed = true;
            resolve({ status: 204 });
          };
        }),
    });
    show();
    await userEvent.click(
      await screen.findByRole("button", { name: "Remove Team Slack" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Remove Team Slack?",
    });
    expect(calls.some((call) => call.method === "DELETE")).toBe(false);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Remove target" }),
    );
    expect(
      await within(dialog).findByRole("button", { name: /Removing/ }),
    ).toBeTruthy();
    answer?.();
    await vi.waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    const list = await screen.findByRole("list", { name: "Targets" });
    await vi.waitFor(() => {
      expect(within(list).queryByText("Team Slack")).toBeNull();
    });
  });

  it("turns a target off with its box, showing the daemon's value", async () => {
    let enabled = true;
    const calls = stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/notify": () => ({
        body: settings({
          targets: [notifyTarget({ enabled, state: enabled ? "on" : "off" })],
        }),
      }),
      "PUT /api/v1/projects/demo/notify/targets/nt-1": () => {
        enabled = false;
        return { body: notifyTarget({ enabled: false, state: "off" }) };
      },
    });
    show();
    const box = await screen.findByRole("checkbox", {
      name: /Send notices to Team Slack/,
    });
    expect(box.getAttribute("aria-checked")).toBe("true");
    await userEvent.click(box);
    await vi.waitFor(() => {
      expect(
        screen
          .getByRole("checkbox", { name: /Send notices to Team Slack/ })
          .getAttribute("aria-checked"),
      ).toBe("false");
    });
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      enabled: false,
    });
  });

  it("shows a plain-text refusal at the URL and keeps the form when the scope changes", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/notify": { body: settings() },
      "POST /api/v1/notify/targets": {
        status: 400,
        body: {
          error: {
            code: "plaintext_notify_target",
            message: "a target without TLS must be on this machine",
            details: {},
          },
        },
      },
    });
    show();
    const form = await screen.findByRole("form", { name: "Add a target" });
    await userEvent.type(within(form).getByLabelText("Name"), "CI");
    await userEvent.type(within(form).getByLabelText(/Webhook URL/), SECURE);
    await userEvent.click(
      screen.getByRole("checkbox", { name: /Add the new target to the host/ }),
    );
    const kept = screen.getByRole("form", { name: "Add a target" });
    expect(within(kept).getByLabelText<HTMLInputElement>("Name").value).toBe(
      "CI",
    );
    await userEvent.click(within(kept).getByRole("button", { name: "Save" }));
    const url = within(kept).getByLabelText(/Webhook URL/);
    await vi.waitFor(() => {
      expect(url.getAttribute("aria-invalid")).toBe("true");
    });
    expect(
      within(kept).getByText(
        "The daemon refused the target: a target without TLS must be on this machine.",
      ),
    ).toBeTruthy();
  });

  it("copies the signing secret of a rotation", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
      "GET /api/v1/projects/demo/notify": { body: settings() },
      "POST /api/v1/notify/targets/nt-2/rotate": {
        body: {
          target: notifyTarget({ id: "nt-2", name: "Release bot", host: true }),
          secret: "whsec_NEW",
        },
      },
    });
    show();
    await userEvent.click(
      await screen.findByRole("button", {
        name: "New signing secret for Release bot",
      }),
    );
    expect(await screen.findByText("whsec_NEW")).toBeTruthy();
    await userEvent.click(
      screen.getByRole("button", { name: "Copy the signing secret" }),
    );
    expect(writeText).toHaveBeenCalledWith("whsec_NEW");
    expect(await screen.findByText("Copied")).toBeTruthy();
  });
});
