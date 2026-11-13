// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { keys } from "@/api/keys";
import { agent } from "@/test/agents";
import { areaEntry, workerList } from "@/test/workers";
import { Markdown } from "@/components/text/Markdown";
import { MentionText } from "./MentionText";

afterEach(cleanup);

function client(): QueryClient {
  const queryClient = new QueryClient();
  queryClient.setQueryData(keys.humans(), ["tanim"]);
  queryClient.setQueryData(keys.agentBar("demo"), [
    agent("frontend"),
    agent("web-lead"),
    agent("main", { tags: ["main"] }),
  ]);
  queryClient.setQueryData(
    keys.workers("demo"),
    workerList({ areas: [areaEntry("web", "frontend", { lead: "web-lead" })] }),
  );
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
    expect(agent?.getAttribute("href")).toBe("/p/demo/agents");
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

  it("opens a lead's area and the main agent's tasks on the board, in text and in Markdown", () => {
    const owner = (key: string) =>
      `/p/demo/board?owner=${encodeURIComponent(JSON.stringify([key]))}`;
    const hrefs = (container: HTMLElement) =>
      [...container.querySelectorAll('a[data-mention="agent"]')].map((a) =>
        a.getAttribute("href"),
      );
    const text = render(
      wrap(<MentionText text="@web-lead @main @frontend" slug="demo" />),
    );
    expect(hrefs(text.container)).toEqual([
      owner("area:web"),
      owner("main"),
      "/p/demo/agents",
    ]);
    cleanup();
    const markdown = render(
      wrap(
        <Markdown mentions={{ slug: "demo" }}>
          {"@web-lead @main @frontend"}
        </Markdown>,
      ),
    );
    expect(hrefs(markdown.container)).toEqual([
      owner("area:web"),
      owner("main"),
      "/p/demo/agents",
    ]);
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
