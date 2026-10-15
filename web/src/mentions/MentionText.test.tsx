// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { keys } from "@/api/keys";
import { Markdown } from "@/features/task/Markdown";
import { MentionText } from "./MentionText";

afterEach(cleanup);

function client(): QueryClient {
  const queryClient = new QueryClient();
  queryClient.setQueryData(keys.humans(), ["tanim"]);
  queryClient.setQueryData(keys.agentBar("demo"), [
    { name: "frontend", workers: [{ name: "frontend" }] },
  ]);
  return queryClient;
}

function wrap(node: React.ReactNode) {
  return <QueryClientProvider client={client()}>{node}</QueryClientProvider>;
}

describe("person mentions", () => {
  it("shows a human in the human colour and an agent as a name in the agent colour, keeping the @", () => {
    const { container } = render(
      wrap(
        <MentionText
          text="ask @tanim and @frontend about @nobody"
          slug="demo"
        />,
      ),
    );
    expect(container.textContent).toContain("ask @tanim and ");
    expect(container.textContent).toContain("about @nobody");
    expect(screen.getByText("@tanim").getAttribute("data-mention")).toBe(
      "human",
    );
    const agent = container.querySelector('a[data-mention="agent"]');
    expect(agent?.getAttribute("href")).toBe("/p/demo/board?agent=frontend");
  });

  it("does the same in Markdown", () => {
    const { container } = render(
      wrap(
        <Markdown mentions={{ slug: "demo" }}>
          {"thanks @tanim and @frontend"}
        </Markdown>,
      ),
    );
    expect(screen.getByText("@tanim").getAttribute("data-mention")).toBe(
      "human",
    );
    expect(container.querySelector('[data-mention="agent"]')?.textContent).toBe(
      "@frontend",
    );
  });
});

describe("item and file mentions", () => {
  it("draw a chip with the icon and without the mark", () => {
    const { container } = render(
      wrap(<MentionText text="see #T-9 and @web/src/a.ts" slug="demo" />),
    );
    const item = container.querySelector('a[data-mention="task"]');
    expect(item?.textContent).toBe("T-9");
    expect(item?.querySelector("svg")).toBeTruthy();
    const file = container.querySelector('a[data-mention="path"]');
    expect(file?.textContent).toBe("web/src/a.ts");
    expect(file?.querySelector("svg")).toBeTruthy();
  });

  it("do the same in Markdown", () => {
    const { container } = render(
      wrap(
        <Markdown mentions={{ slug: "demo" }}>
          {"see #T-9 and @web/src/a.ts"}
        </Markdown>,
      ),
    );
    expect(container.querySelector('[data-mention="task"]')?.textContent).toBe(
      "T-9",
    );
    expect(container.querySelector('[data-mention="path"]')?.textContent).toBe(
      "web/src/a.ts",
    );
  });
});
