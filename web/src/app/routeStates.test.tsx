// @vitest-environment jsdom
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { ApiError } from "../api/errors";
import { RouteError, RoutePending } from "./routeStates";

afterEach(cleanup);

function setup(loader: () => Promise<null>) {
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    loader,
    component: () => <p>Loaded</p>,
  });
  return createRouter({
    routeTree: root.addChildren([index]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
    defaultPendingComponent: RoutePending,
    defaultErrorComponent: RouteError,
    defaultPendingMs: 0,
    defaultPendingMinMs: 0,
  });
}

describe("route states", () => {
  it("shows the skeleton while the loader runs", async () => {
    const router = setup(() => new Promise<null>(() => {}));
    render(<RouterProvider router={router} />);
    const skeleton = await screen.findByRole("status", { name: "Loading" });
    expect(skeleton.getAttribute("aria-busy")).toBe("true");
  });

  it("shows the error banner and Retry re-runs the loader", async () => {
    let calls = 0;
    const router = setup(() => {
      calls += 1;
      return calls === 1
        ? Promise.reject(new Error("boom"))
        : Promise.resolve(null);
    });
    render(<RouterProvider router={router} />);
    expect((await screen.findByRole("alert")).textContent).toContain(
      "could not load",
    );
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Loaded")).toBeTruthy();
    expect(calls).toBe(2);
  });

  it("shows the project not-found state with the way back to the list for a project fold does not know", async () => {
    const router = setup(() =>
      Promise.reject(new ApiError("project_not_found", "removed")),
    );
    render(<RouterProvider router={router} />);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Project not found");
    expect(alert.textContent).toContain("code: project_not_found");
    expect(alert.textContent).not.toContain("did not arrive");
    expect(
      screen
        .getByRole("link", { name: "Back to projects" })
        .getAttribute("href"),
    ).toBe("/projects");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("says what to do for a project whose database is missing, and Retry asks again", async () => {
    let calls = 0;
    const router = setup(() => {
      calls += 1;
      return Promise.reject(
        new ApiError("project_unavailable", "unavailable", {
          reason: "database_missing",
          hint: "restore /srv/app/.fold/fold.db from a backup",
        }),
      );
    });
    render(<RouterProvider router={router} />);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Project database missing");
    expect(alert.textContent).toContain(
      "restore /srv/app/.fold/fold.db from a backup",
    );
    expect(alert.textContent).toContain("code: project_unavailable");
    expect(alert.textContent).not.toContain("did not arrive");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByRole("alert");
    expect(calls).toBe(2);
  });
});
