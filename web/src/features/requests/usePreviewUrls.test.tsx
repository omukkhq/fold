// @vitest-environment jsdom
import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { usePreviewUrls } from "./usePreviewUrls";

const create = vi.fn();
const revoke = vi.fn();

beforeEach(() => {
  let n = 0;
  create.mockImplementation(() => `blob:${String((n += 1))}`);
  Object.defineProperty(URL, "createObjectURL", {
    value: create,
    configurable: true,
  });
  Object.defineProperty(URL, "revokeObjectURL", {
    value: revoke,
    configurable: true,
  });
});

afterEach(() => {
  cleanup();
  create.mockReset();
  revoke.mockReset();
  Reflect.deleteProperty(URL, "createObjectURL");
  Reflect.deleteProperty(URL, "revokeObjectURL");
});

describe("usePreviewUrls", () => {
  it("gives a URL to a picked image, revokes it when the file leaves and when the form unmounts", () => {
    const png = new File(["x"], "a.png", { type: "image/png" });
    const gif = new File(["x"], "b.gif", { type: "image/gif" });
    const { result, rerender, unmount } = renderHook(
      ({ files }: { files: File[] }) => usePreviewUrls(files),
      { initialProps: { files: [png, gif] } },
    );
    expect(result.current.get(png)).toBe("blob:1");
    rerender({ files: [gif] });
    expect(revoke).toHaveBeenCalledWith("blob:1");
    expect(result.current.has(png)).toBe(false);
    unmount();
    expect(revoke).toHaveBeenCalledWith("blob:2");
    expect(revoke).toHaveBeenCalledTimes(2);
  });
});
