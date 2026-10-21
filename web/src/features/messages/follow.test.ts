import { describe, expect, it } from "vitest";
import { FOLLOW_SLACK, isFollowing } from "./follow";

const view = (scrollTop: number) => ({
  scrollTop,
  clientHeight: 400,
  scrollHeight: 1000,
});

describe("isFollowing", () => {
  it("follows at the bottom", () => {
    expect(isFollowing(view(600))).toBe(true);
  });
  it("follows within the slack", () => {
    expect(isFollowing(view(600 - FOLLOW_SLACK))).toBe(true);
  });
  it("does not follow beyond the slack", () => {
    expect(isFollowing(view(600 - FOLLOW_SLACK - 1))).toBe(false);
  });
  it("follows a thread that does not scroll", () => {
    expect(
      isFollowing({ scrollTop: 0, clientHeight: 400, scrollHeight: 300 }),
    ).toBe(true);
  });
});
