import { describe, expect, it } from "vitest";
import { initials } from "./initials";

describe("initials", () => {
  it.each([
    ["frontend/ui", "FU"],
    ["frontend", "F"],
    ["human:tanim", "T"],
    ["human:tanim-rafeed", "TR"],
    ["agent:backend/api/v2", "BA"],
    ["review_bot", "RB"],
    ["élan", "É"],
    ["a//b", "AB"],
  ])("turns %s into %s", (name, letters) => {
    expect(initials(name)).toBe(letters);
  });

  it("shows a question mark when there is no letter", () => {
    expect(initials("")).toBe("?");
    expect(initials("///")).toBe("?");
    expect(initials("human:")).toBe("?");
  });
});
