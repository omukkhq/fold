import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { LiveRetryContext, useLiveRetry } from "./retry";

describe("useLiveRetry", () => {
  it("does nothing outside a live project and calls the provided retry inside one", () => {
    const seen: (() => void)[] = [];
    function Probe() {
      seen.push(useLiveRetry());
      return null;
    }
    renderToString(createElement(Probe));
    const retry = vi.fn();
    renderToString(
      createElement(LiveRetryContext, { value: retry }, createElement(Probe)),
    );
    expect(() => seen[0]?.()).not.toThrow();
    seen[1]?.();
    expect(retry).toHaveBeenCalledTimes(1);
  });
});
