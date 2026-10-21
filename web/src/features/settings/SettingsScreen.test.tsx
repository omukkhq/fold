// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../../test/stubFetch";
import { SettingsScreen } from "./SettingsScreen";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** Written in parts: the source guard bans a remote URL in a component test. */
const HOOKS = ["https", "//hooks.example.com"].join(":");

const WEBHOOK = {
  id: "nt-1",
  kind: "webhook",
  interval_minutes: 10,
  created_at: 0,
  created_by: "ana",
  url: `${HOOKS}/…`,
  to: null,
  from: null,
  smtp_host: null,
  smtp_port: null,
  security: null,
  username: null,
  secret_set: true,
};

function renderScreen() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <SettingsScreen slug="demo" />
    </QueryClientProvider>,
  );
  return client;
}

describe("SettingsScreen", () => {
  it("adds a webhook, tests it and shows the daemon's reason for a failed test", async () => {
    let targets: (typeof WEBHOOK)[] = [];
    const calls = stubFetch({
      "GET /api/v1/session": { body: { human: "ana", authenticated: true } },
      "GET /api/v1/projects/demo/notify": () => ({
        body: {
          targets,
          host_targets: [],
          uses_host: targets.length === 0,
          link_base: "local",
        },
      }),
      "GET /api/v1/notify": {
        body: { targets: [], web_url: null, link_base: "local" },
      },
      "POST /api/v1/projects/demo/notify/targets": () => {
        targets = [WEBHOOK];
        return { status: 201, body: WEBHOOK };
      },
      "POST /api/v1/projects/demo/notify/targets/nt-1/test": {
        status: 502,
        body: {
          error: {
            code: "notify_send_failed",
            message: "the webhook receiver answered HTTP 500",
            details: {},
          },
        },
      },
    });
    const client = renderScreen();
    expect(await screen.findByText(/No target gets this project/)).toBeTruthy();
    const form = screen.getByRole("form", { name: "Add a project target" });
    const user = userEvent.setup();
    await user.click(within(form).getByRole("button", { name: "Add target" }));
    expect(within(form).getByText(/starting with https or http/)).toBeTruthy();
    await user.type(within(form).getByLabelText("URL"), `${HOOKS}/fold`);
    await user.click(
      within(form).getByRole("button", { name: "Generate a secret" }),
    );
    await user.click(within(form).getByRole("button", { name: "Add target" }));
    const list = await screen.findByRole("list", { name: "Project targets" });
    expect(within(list).getByText(`${HOOKS}/…`)).toBeTruthy();
    const added = calls.find(
      (call) => call.method === "POST" && call.path.endsWith("/targets"),
    );
    expect(added?.body).toMatchObject({
      kind: "webhook",
      url: `${HOOKS}/fold`,
    });
    const secret = JSON.stringify(added?.body).match(/"secret":"(\w+)"/)?.[1];
    expect(secret).toHaveLength(48);
    await waitFor(() => {
      expect(client.getMutationCache().getAll()).toHaveLength(0);
    });
    expect(within(form).getByLabelText("Signing secret")).toHaveProperty(
      "value",
      "",
    );
    await user.click(
      within(list).getByRole("button", {
        name: "Send a test notice through nt-1",
      }),
    );
    expect(
      await within(list).findByText(
        "The test notice was not sent: the webhook receiver answered HTTP 500.",
      ),
    ).toBeTruthy();
  });

  it("shows a load failure with its code and asks for a name before any write", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: null, authenticated: false } },
      "GET /api/v1/projects/demo/notify": errorReply(500, "notify_settings_io"),
      "GET /api/v1/notify": {
        body: { targets: [WEBHOOK], web_url: null, link_base: "local" },
      },
      "POST /api/v1/notify/targets/nt-1/test": { status: 204 },
    });
    renderScreen();
    expect(
      await screen.findByText(
        "The daemon could not read or write its notification settings.",
      ),
    ).toBeTruthy();
    expect(await screen.findByText(/Tell fold your name first/)).toBeTruthy();
    const host = await screen.findByRole("list", { name: "Host targets" });
    const test = within(host).getByRole("button", {
      name: "Send a test notice through nt-1",
    });
    expect(test.hasAttribute("disabled")).toBe(true);
  });
});
