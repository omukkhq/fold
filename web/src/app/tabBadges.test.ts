import { describe, expect, it } from "vitest";
import { tabBadge } from "./tabBadges";

describe("tabBadge", () => {
  it("counts the proposals that wait for the human's review", () => {
    expect(tabBadge("proposals", 1, 4)).toMatchObject({
      count: 1,
      label: "1 proposal waits for your review",
    });
    expect(tabBadge("proposals", 2, 0)?.label).toBe(
      "2 proposals wait for your review",
    );
  });

  it("counts the threads that wait for the human's answer", () => {
    expect(tabBadge("messages", 3, 1)).toMatchObject({
      count: 1,
      label: "1 thread waits for your answer",
    });
    expect(tabBadge("messages", 0, 2)?.label).toBe(
      "2 threads wait for your answer",
    );
  });

  it("shows no count at zero or on a tab that counts nothing", () => {
    expect(tabBadge("proposals", 0, 5)).toBeNull();
    expect(tabBadge("messages", 5, 0)).toBeNull();
    expect(tabBadge("board", 5, 5)).toBeNull();
  });
});
