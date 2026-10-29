/** The `lg` layout, where the thread list and the open thread show side by side. */
export const WIDE_LAYOUT = "(min-width: 64rem)";

/** The thread pane is on the screen: the address names a thread, or the wide layout shows the newest visible thread beside the list. A narrow screen with no thread in the address shows only the list. */
export function paneShown(
  requested: string | undefined,
  wide: boolean,
): boolean {
  return requested !== undefined || wide;
}

export interface ReadGate {
  /** The newest message of the thread that waits for the signed-in human to read it. */
  unread: string | null;
  /** The thread pane is on the screen. */
  shown: boolean;
  /** The browser tab is in the foreground. */
  visible: boolean;
  /** The newest unread message of the last mark-read call. */
  marked: string | null;
}

/** The message a mark-read call is due for, or null: nothing waits, the human cannot see the thread, or a call for that message was made already, so a refused call is not repeated until another message arrives. */
export function readDue({
  unread,
  shown,
  visible,
  marked,
}: ReadGate): string | null {
  if (unread === null || !shown || !visible || unread === marked) {
    return null;
  }
  return unread;
}

/** One mark-read call: the thread and its newest message that waits for the signed-in human. */
export interface ReadCall {
  thread: string;
  unread: string;
}

/** The mark-read calls due for a chat's threads, one per thread whose newest unread message has no call yet; none while the human cannot see the chat. */
export function readsDue(
  unread: readonly ReadCall[],
  shown: boolean,
  visible: boolean,
  marked: ReadonlyMap<string, string>,
): ReadCall[] {
  return unread.filter(
    (call) =>
      readDue({
        unread: call.unread,
        shown,
        visible,
        marked: marked.get(call.thread) ?? null,
      }) !== null,
  );
}
