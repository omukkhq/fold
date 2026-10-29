export type InboxTab = "needs" | "updates";

/** The Inbox opens on what waits for the human; with nothing waiting it opens on the card moves. */
export function inboxTab(waiting: number): InboxTab {
  return waiting > 0 ? "needs" : "updates";
}

/** The Inbox button's accessible name: the gold count is what waits, and new card moves are named after it. */
export function inboxLabel(waiting: number, unread: number): string {
  const parts = ["Inbox"];
  if (waiting > 0) {
    parts.push(
      `${String(waiting)} ${waiting === 1 ? "waits" : "wait"} for you`,
    );
  }
  if (unread > 0) {
    parts.push(`${String(unread)} new ${unread === 1 ? "update" : "updates"}`);
  }
  return parts.join(", ");
}
