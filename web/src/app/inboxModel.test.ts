import { describe, expect, it } from "vitest";
import { inboxLabel, inboxTab } from "./inboxModel";

describe("inboxTab", () => {
  it("opens on what waits, else on the card moves", () => {
    expect(inboxTab(2)).toBe("needs");
    expect(inboxTab(0)).toBe("updates");
  });
});

describe("inboxLabel", () => {
  it("names what waits and the new card moves", () => {
    expect(inboxLabel(0, 0)).toBe("Inbox");
    expect(inboxLabel(1, 0)).toBe("Inbox, 1 waits for you");
    expect(inboxLabel(9, 1)).toBe("Inbox, 9 wait for you, 1 new update");
    expect(inboxLabel(0, 3)).toBe("Inbox, 3 new updates");
  });
});
