// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import { keys } from "@/api/keys";
import { usePeople } from "./usePeople";

function wrapper(client: QueryClient) {
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

describe("usePeople", () => {
  it("joins the cached humans with every agent and worker name", () => {
    const client = new QueryClient();
    client.setQueryData(keys.humans(), ["tanim"]);
    client.setQueryData(keys.agentBar("demo"), [
      { name: "frontend", workers: [{ name: "frontend/ui" }] },
    ]);
    const { result } = renderHook(() => usePeople("demo"), {
      wrapper: wrapper(client),
    });
    expect(result.current).toEqual({
      humans: ["tanim"],
      agents: ["frontend", "frontend/ui"],
    });
  });

  it("knows nobody without a slug and before the lists arrive", () => {
    const client = new QueryClient({
      defaultOptions: { queries: { queryFn: () => new Promise(() => {}) } },
    });
    const none = renderHook(() => usePeople(null), {
      wrapper: wrapper(client),
    });
    expect(none.result.current).toEqual({ humans: [], agents: [] });
    expect(client.getQueryCache().getAll()).toHaveLength(2);
    expect(
      client
        .getQueryCache()
        .getAll()
        .every((q) => q.state.fetchStatus === "idle"),
    ).toBe(true);
  });
});
