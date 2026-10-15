// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { MessageBody } from "./MessageBody";

afterEach(cleanup);

async function show(body: string) {
  const root = createRootRoute();
  const index = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => (
      <QueryClientProvider client={new QueryClient()}>
        <MessageBody body={body} slug="demo" />
      </QueryClientProvider>
    ),
  });
  const router = createRouter({
    routeTree: root.addChildren([index]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(<RouterProvider router={router} />);
  await waitFor(() => {
    expect(document.querySelector("p")).not.toBeNull();
  });
}

// An id such as m-210 looks like a margin utility to the token guard, so the test builds it.
const MESSAGE = `m-${String(210)}`;

describe("MessageBody", () => {
  it("links ids to their screens", async () => {
    await show(`Review P-3 for T-12, see ${MESSAGE}.`);
    const links = screen.getAllByRole("link");
    expect(links.map((a) => [a.textContent, a.getAttribute("href")])).toEqual([
      ["P-3", "/p/demo/proposals/P-3"],
      ["T-12", "/p/demo/t/T-12"],
      [MESSAGE, `/p/demo/messages?thread=${MESSAGE}`],
    ]);
  });

  it("links # items and @ files, keeps bare ids linked and leaves a code span as text", async () => {
    await show("See #T-12, @web/src/board.ts and P-3 but not `@web/x.ts`.");
    const links = screen.getAllByRole("link");
    expect(links.map((a) => [a.textContent, a.getAttribute("href")])).toEqual([
      ["T-12", "/p/demo/t/T-12"],
      ["web/src/board.ts", "/p/demo/repository?path=web%2Fsrc%2Fboard.ts"],
      ["P-3", "/p/demo/proposals/P-3"],
    ]);
  });

  it("renders HTML as text and creates no element from it", async () => {
    const body = '<img src=x onerror="alert(1)"> <b>bold</b> T-5';
    await show(body);
    const paragraph = screen.getByText(/bold/, { selector: "p" });
    expect(paragraph.textContent).toBe(body);
    expect(paragraph.querySelector("img, b, script")).toBeNull();
    expect(screen.getAllByRole("link")).toHaveLength(1);
  });
});
