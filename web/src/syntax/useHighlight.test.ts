// @vitest-environment jsdom
import { cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { SyntaxLine } from "./highlight";
import { syntaxColor } from "./theme";
import {
  highlightRequest,
  useHighlight,
  type HighlightRequest,
} from "./useHighlight";

afterEach(cleanup);

describe("highlightRequest", () => {
  it("keys the request by its language and text", () => {
    const a = highlightRequest("fence:rust", [["fn a()"], ["fn b()"]]);
    expect(highlightRequest("fence:rust", [["fn a()"], ["fn b()"]]).key).toBe(
      a.key,
    );
    expect(highlightRequest("fence:go", [["fn a()"], ["fn b()"]]).key).not.toBe(
      a.key,
    );
    expect(highlightRequest("fence:rust", [["fn a()", "fn b()"]]).key).not.toBe(
      a.key,
    );
  });
});

describe("useHighlight", () => {
  it("is null until the tokens are ready", async () => {
    const { result } = renderHook(() =>
      useHighlight(highlightRequest("fence:typescript", [["let x = 1"]])),
    );
    expect(result.current).toBeNull();
    await waitFor(() => {
      expect(result.current?.[0]?.[0]?.[0]).toEqual({
        content: "let",
        color: syntaxColor("keyword"),
      });
    });
  });

  it("never shows the old code's tokens for new code", async () => {
    const { result, rerender } = renderHook<
      SyntaxLine[][] | null,
      { request: HighlightRequest | null }
    >(({ request }) => useHighlight(request), {
      initialProps: {
        request: highlightRequest("fence:typescript", [["let x = 1"]]),
      },
    });
    await waitFor(() => {
      expect(result.current).not.toBeNull();
    });
    rerender({
      request: highlightRequest("fence:typescript", [["const y = 2"]]),
    });
    expect(result.current).toBeNull();
    await waitFor(() => {
      expect(result.current?.[0]?.[0]?.[0]?.content).toBe("const");
    });
    rerender({ request: null });
    expect(result.current).toBeNull();
  });

  it("stays null for code without a language", () => {
    const { result } = renderHook(() =>
      useHighlight(highlightRequest(null, [["let x = 1"]])),
    );
    expect(result.current).toBeNull();
  });
});
