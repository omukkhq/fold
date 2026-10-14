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
});
