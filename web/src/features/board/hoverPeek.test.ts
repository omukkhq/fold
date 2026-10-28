import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CLOSE_DELAY_MS, OPEN_DELAY_MS, hoverPeek } from "./hoverPeek";

function setup() {
  const changes: boolean[] = [];
  const peek = hoverPeek(
    (open) => {
      changes.push(open);
    },
    {
      set: (run, ms) => Number(setTimeout(run, ms)),
      clear: (id) => {
        clearTimeout(id);
      },
    },
  );
  return { peek, changes };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("hoverPeek", () => {
  it("opens once the pointer rests on the card, and not when it only passes", () => {
    const { peek, changes } = setup();
    peek.pointerEnter("mouse");
    vi.advanceTimersByTime(OPEN_DELAY_MS - 1);
    peek.pointerLeave();
    vi.advanceTimersByTime(OPEN_DELAY_MS);
    expect(changes).toEqual([]);
    peek.pointerEnter("mouse");
    vi.advanceTimersByTime(OPEN_DELAY_MS);
    expect(changes).toEqual([true]);
  });

  it("keeps the panel while the pointer crosses into it, and closes after the grace", () => {
    const { peek, changes } = setup();
    peek.pointerEnter("pen");
    vi.advanceTimersByTime(OPEN_DELAY_MS);
    peek.pointerLeave();
    vi.advanceTimersByTime(CLOSE_DELAY_MS - 1);
    peek.pointerEnter("mouse");
    vi.advanceTimersByTime(CLOSE_DELAY_MS);
    expect(changes).toEqual([true]);
    peek.pointerLeave();
    vi.advanceTimersByTime(CLOSE_DELAY_MS);
    expect(changes).toEqual([true, false]);
  });

  it("never opens for touch", () => {
    const { peek, changes } = setup();
    peek.pointerEnter("touch");
    vi.advanceTimersByTime(OPEN_DELAY_MS);
    expect(changes).toEqual([]);
  });

  it("opens at once on focus and stays while focus moves inside", () => {
    const { peek, changes } = setup();
    peek.focusIn();
    peek.focusOut(true);
    expect(changes).toEqual([true]);
    peek.pointerEnter("mouse");
    peek.pointerLeave();
    vi.advanceTimersByTime(CLOSE_DELAY_MS);
    expect(changes).toEqual([true]);
    peek.focusOut(false);
    expect(changes).toEqual([true, false]);
  });

  it("stays open on blur while the pointer rests on it", () => {
    const { peek, changes } = setup();
    peek.focusIn();
    peek.pointerEnter("mouse");
    peek.focusOut(false);
    expect(changes).toEqual([true]);
    peek.dismiss();
    expect(changes).toEqual([true, false]);
  });

  it("does not open for the focus a press on the card brings, and opens for the next focus", () => {
    const { peek, changes } = setup();
    peek.cardPress();
    peek.focusIn();
    expect(changes).toEqual([]);
    peek.focusOut(false);
    peek.focusIn();
    expect(changes).toEqual([true]);
  });

  it("forgets a press on the card that brought no focus once the pointer leaves", () => {
    const { peek, changes } = setup();
    peek.cardPress();
    peek.pointerLeave();
    peek.focusIn();
    expect(changes).toEqual([true]);
  });

  it("holds the panel after a press inside it until the pointer comes back and leaves, or a dismiss", () => {
    const { peek, changes } = setup();
    peek.press();
    peek.pointerEnter("mouse");
    vi.advanceTimersByTime(OPEN_DELAY_MS);
    peek.press();
    peek.focusIn();
    peek.focusOut(false);
    peek.pointerLeave();
    vi.advanceTimersByTime(CLOSE_DELAY_MS);
    expect(changes).toEqual([true]);
    peek.pointerEnter("mouse");
    peek.pointerLeave();
    vi.advanceTimersByTime(CLOSE_DELAY_MS);
    expect(changes).toEqual([true, false]);
    peek.pointerEnter("mouse");
    vi.advanceTimersByTime(OPEN_DELAY_MS);
    peek.press();
    peek.pointerLeave();
    peek.dismiss();
    expect(changes).toEqual([true, false, true, false]);
    peek.pointerEnter("mouse");
    peek.pointerLeave();
    vi.advanceTimersByTime(OPEN_DELAY_MS);
    expect(changes).toEqual([true, false, true, false]);
  });

  it("drops a pending open when stopped", () => {
    const { peek, changes } = setup();
    peek.pointerEnter("mouse");
    peek.stop();
    vi.advanceTimersByTime(OPEN_DELAY_MS);
    expect(changes).toEqual([]);
    peek.stop();
  });
});
