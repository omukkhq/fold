/** A pointer must rest on a card this long before its detail panel opens, so sweeping across the board opens none. */
export const OPEN_DELAY_MS = 250;
/** The grace after the pointer leaves the card, so it can cross the gap into the panel. */
export const CLOSE_DELAY_MS = 150;

export interface Timers {
  set: (run: () => void, ms: number) => number;
  clear: (id: number) => void;
}

export interface HoverPeek {
  /** A mouse or pen entered the card or its panel; touch never opens the panel. */
  pointerEnter: (pointer: string) => void;
  pointerLeave: () => void;
  /** Keyboard focus reached the card's link or its panel. */
  focusIn: () => void;
  /** Focus left; `inside` tells whether it moved to another element of the card or its panel. */
  focusOut: (inside: boolean) => void;
  /** Escape, or a press outside the card. */
  dismiss: () => void;
  /** Drops a pending timer when the card unmounts. */
  stop: () => void;
}

/**
 * When a card's detail panel shows: after the pointer rests on the card, at once on keyboard focus, and until both the
 * pointer and the focus have left the card and its panel.
 */
export function hoverPeek(
  onChange: (open: boolean) => void,
  timers: Timers,
): HoverPeek {
  let open = false;
  let hovered = false;
  let focused = false;
  let timer: number | null = null;

  const cancel = () => {
    if (timer !== null) {
      timers.clear(timer);
      timer = null;
    }
  };
  const show = (next: boolean) => {
    cancel();
    if (open !== next) {
      open = next;
      onChange(next);
    }
  };
  const later = (next: boolean, ms: number) => {
    cancel();
    timer = timers.set(() => {
      timer = null;
      show(next);
    }, ms);
  };

  return {
    pointerEnter: (pointer) => {
      if (pointer === "touch") {
        return;
      }
      hovered = true;
      if (open) {
        cancel();
      } else {
        later(true, OPEN_DELAY_MS);
      }
    },
    pointerLeave: () => {
      hovered = false;
      if (focused) {
        return;
      }
      if (open) {
        later(false, CLOSE_DELAY_MS);
      } else {
        cancel();
      }
    },
    focusIn: () => {
      focused = true;
      show(true);
    },
    focusOut: (inside) => {
      if (inside) {
        return;
      }
      focused = false;
      if (!hovered) {
        show(false);
      }
    },
    dismiss: () => {
      hovered = false;
      focused = false;
      show(false);
    },
    stop: cancel,
  };
}
