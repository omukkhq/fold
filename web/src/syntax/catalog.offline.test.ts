import { describe, expect, it, vi } from "vitest";
import { highlightSegments } from "./highlight";

vi.mock("./catalog", () => {
  throw new Error("offline");
});

describe("highlightSegments without the catalog", () => {
  it("keeps the code plain and shows no error when the catalog fails to load", async () => {
    const error = vi.spyOn(console, "error");
    expect(
      await highlightSegments("fence:rust", [["fn main() {}"]]),
    ).toBeNull();
    expect(
      await highlightSegments("fence:rust", [["fn main() {}"]]),
    ).toBeNull();
    expect(error).not.toHaveBeenCalled();
  });
});
