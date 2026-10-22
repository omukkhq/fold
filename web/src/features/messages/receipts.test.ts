import { describe, expect, it } from "vitest";
import type { RecipientView } from "@/api/messages";
import { segmentsText } from "@/people/segments";
import { message, recipient } from "@/test/messages";
import { detailRow, receipts } from "./receipts";

const NOW = new Date(2026, 9, 22, 22, 30).getTime();
const AT = new Date(2026, 9, 22, 22, 19).getTime();

function line(recipients: RecipientView[], you: string | null = "tanim") {
  const result = receipts(
    message("m-1", { sender: "web-worker", recipients }),
    NOW,
    you,
  );
  return {
    ...result,
    text: result.parts.map((part) => segmentsText(part.segments)),
    tones: result.parts.map((part) => part.tone),
  };
}

describe("receipts", () => {
  it("has no line for a message without recipients", () => {
    expect(line([])).toMatchObject({ parts: [], folded: false, details: [] });
  });

  it("names each of two recipients, the read before the delivered", () => {
    const result = line([
      recipient("human:rafeed", { delivered_at: AT }),
      recipient("human:tanim", { acknowledged_at: AT }),
    ]);
    expect(result.folded).toBe(false);
    expect(result.text).toEqual([
      "Read by @tanim · 22:19",
      "Delivered to @rafeed · 22:19",
    ]);
    expect(result.tones).toEqual(["read", "delivered"]);
  });

  it("leads with a failed or waiting recipient and keeps its note", () => {
    expect(
      line([
        recipient("web-lead", { acknowledged_at: AT }),
        recipient("backend", { pushed_at: AT }),
      ]).text,
    ).toEqual([
      "Waiting for @backend · 22:19 · it is busy until its current step ends",
      "Acknowledged by @web-lead · 22:19",
    ]);
    expect(
      line([recipient("human:rafeed"), recipient("review")]).tones,
    ).toEqual(["failed", "delivered"]);
  });

  it("words a stopped worker without a lead word", () => {
    expect(
      line([recipient("web-worker-41", { forwarded_to: "web-lead" })]).text,
    ).toEqual(["@web-worker-41 cannot start now · sent on to @web-lead"]);
  });

  it("folds many recipients into counts, you first in your group", () => {
    const result = line([
      recipient("human:rafeed", { acknowledged_at: AT }),
      recipient("human:tanim", { acknowledged_at: AT }),
      recipient("human:mitu", { acknowledged_at: AT }),
      recipient("frontend", { delivered_at: AT }),
      recipient("backend", { delivered_at: AT }),
    ]);
    expect(result.folded).toBe(true);
    expect(result.text).toEqual([
      "Read by @tanim and 2 others",
      "delivered to 2",
    ]);
    expect(result.details).toHaveLength(5);
  });

  it("folds with the worst state first and names a lone recipient of a state", () => {
    const result = line([
      recipient("human:tanim", { acknowledged_at: AT }),
      recipient("human:rafeed", { acknowledged_at: AT }),
      recipient("review"),
      recipient("web-worker-1", { forwarded_to: "web-lead" }),
      recipient("web-worker-2", { forwarded_to: "web-lead" }),
      recipient("human:mitu"),
    ]);
    expect(result.text).toEqual([
      "Not delivered to @review",
      "2 cannot start now",
      "read by @tanim and 1 other",
      "sent to @mitu",
    ]);
    expect(result.tones).toEqual(["failed", "forwarded", "read", "delivered"]);
  });

  it("counts every recipient when no one is signed in, apart by lead word", () => {
    const result = line(
      [
        recipient("human:rafeed", { acknowledged_at: AT }),
        recipient("human:tanim", { acknowledged_at: AT }),
        recipient("frontend", { acknowledged_at: AT }),
      ],
      null,
    );
    expect(result.text).toEqual(["Read by 2", "acknowledged by @frontend"]);
  });
});

describe("detailRow", () => {
  it("puts the time and the note at the right of a delivered or waiting recipient", () => {
    const [waiting, read] = line([
      recipient("backend", { pushed_at: AT }),
      recipient("human:tanim", { acknowledged_at: AT }),
    ]).details.map(detailRow);
    expect(waiting).toEqual({
      tone: "waiting",
      lead: "Waiting for",
      actor: "backend",
      after: null,
      trailing: "22:19 · it is busy until its current step ends",
    });
    expect(read).toMatchObject({ lead: "Read by", trailing: "22:19" });
  });
  it("puts a stopped worker's note after its name", () => {
    const [row] = line([
      recipient("web-worker-41", { forwarded_to: "web-lead" }),
    ]).details.map(detailRow);
    expect(row).toEqual({
      tone: "forwarded",
      lead: "",
      actor: "web-worker-41",
      after: "cannot start now · sent on to @web-lead",
      trailing: "",
    });
  });
});
