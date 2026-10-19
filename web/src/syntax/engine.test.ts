import { afterEach, describe, expect, it, vi } from "vitest";
import { highlightSegments } from "./highlight";
import { ENGINE } from "./shiki";
import { syntaxColor } from "./theme";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

// The engine loads once per page, so these steps run in order in one test of their own file.
describe("the Oniguruma engine", () => {
  it("loads on the first code with a grammar, once, and again after a failed load, with no error shown", async () => {
    const wasm = vi.spyOn(ENGINE, "wasm");
    const error = vi.spyOn(console, "error");

    expect(await highlightSegments("file:notes.txt", [["x"]])).toBeNull();
    expect(wasm).not.toHaveBeenCalled();

    wasm.mockRejectedValueOnce(new Error("offline"));
    expect(
      await highlightSegments("fence:rust", [["fn main() {}"]]),
    ).toBeNull();

    const rust = await highlightSegments("fence:rust", [["fn main() {}"]]);
    expect(rust?.[0]?.[0]?.[0]).toEqual({
      content: "fn",
      color: syntaxColor("keyword"),
    });
    const go = await highlightSegments("fence:go", [["func main() {}"]]);
    expect(go?.[0]?.[0]?.[0]?.content).toBe("func");
    expect(wasm).toHaveBeenCalledTimes(2);
    expect(error).not.toHaveBeenCalled();
  });

  it("fails to load when the bundle does not answer with the file", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.resolve(new Response(null, { status: 404 })),
    );
    await expect(ENGINE.wasm()).rejects.toThrow("HTTP 404");
  });
});
