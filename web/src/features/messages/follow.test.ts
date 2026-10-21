import { describe, expect, it } from "vitest";
import {
  FOLLOW_SLACK,
  followNewest,
  followResizes,
  isFollowing,
  toBottom,
} from "./follow";

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

class FakeObserver {
  static last: FakeObserver | null = null;
  observed: string[] = [];
  disconnected = false;
  constructor(readonly fire: () => void) {
    FakeObserver.last = this;
  }
  observe(target: string) {
    this.observed.push(target);
  }
  disconnect() {
    this.disconnected = true;
  }
}

function observed(): FakeObserver {
  if (FakeObserver.last === null) {
    throw new Error("no observer");
  }
  return FakeObserver.last;
}

describe("toBottom", () => {
  it("scrolls to the newest message", () => {
    const box = view(0);
    toBottom(box);
    expect(box.scrollTop).toBe(1000);
  });
});

describe("followNewest", () => {
  it("follows again and scrolls to the reader's own message", () => {
    const box = view(100);
    const following = { current: false };
    followNewest(box, following);
    expect(following.current).toBe(true);
    expect(box.scrollTop).toBe(1000);
  });
  it("follows again before the thread has a scroller", () => {
    const following = { current: false };
    followNewest(null, following);
    expect(following.current).toBe(true);
  });
});

describe("followResizes", () => {
  it("goes back to the bottom on a resize while following", () => {
    const box = view(0);
    const stop = followResizes(
      box,
      ["box", "list"],
      { current: true },
      FakeObserver,
    );
    expect(observed().observed).toEqual(["box", "list"]);
    observed().fire();
    expect(box.scrollTop).toBe(1000);
    stop();
    expect(observed().disconnected).toBe(true);
  });
  it("keeps the place of a reader who scrolled up", () => {
    const box = view(100);
    followResizes(box, ["box"], { current: false }, FakeObserver);
    observed().fire();
    expect(box.scrollTop).toBe(100);
  });
  it("follows again once a hidden thread is shown", () => {
    const box = { scrollTop: 0, clientHeight: 0, scrollHeight: 0 };
    const following = { current: false };
    followResizes(box, ["box"], following, FakeObserver);
    observed().fire();
    expect(following.current).toBe(true);
    box.clientHeight = 400;
    box.scrollHeight = 1000;
    observed().fire();
    expect(box.scrollTop).toBe(1000);
  });
  it("does nothing where the browser has no resize observer", () => {
    const box = view(100);
    followResizes(box, ["box"], { current: true }, undefined)();
    expect(box.scrollTop).toBe(100);
  });
});
