import { describe, expect, it } from "vitest";
import type { MessageView } from "@/api/messages";
import { areaEntry, workerProcess } from "@/test/workers";
import {
  MAX_INSTRUCTIONS,
  agentFacts,
  chatLines,
  instructionsChange,
  instructionsSchema,
  ledArea,
  noteDraft,
  parseChatSearch,
} from "./chat";

function message(id: string, sender: string): MessageView {
  return {
    id,
    thread: "m-1",
    reply_to: null,
    sender,
    task: null,
    proposal: null,
    request: null,
    body: "text",
    sent_at: 0,
    recipients: [],
    choices: [],
    header: null,
  };
}

describe("chatLines", () => {
  it("puts the humans' messages on their own side", () => {
    const lines = chatLines([
      message("m-1", "human:ana"),
      message("m-2", "web-lead"),
    ]);
    expect(lines.map((line) => line.fromHuman)).toEqual([true, false]);
    expect(lines[1]?.message.id).toBe("m-2");
  });
});

describe("ledArea", () => {
  it("finds the area the agent leads", () => {
    const areas = [
      areaEntry("api", "backend", { lead: "api-lead" }),
      areaEntry("web", "frontend", { lead: "web-lead" }),
    ];
    expect(ledArea(areas, "web-lead")?.name).toBe("web");
    expect(ledArea(areas, "main")).toBeNull();
  });
});

describe("agentFacts", () => {
  it("names the role and how a message reaches the agent", () => {
    const workers = [
      workerProcess("web-lead"),
      workerProcess("review", { role: "review_lead", area: null }),
    ];
    const lead = agentFacts(workers, "web-lead");
    expect(lead.role).toBe("area lead of web");
    expect(lead.delivery).toContain("as a turn");
    expect(lead.delivery).toContain("goes to the main agent");
    expect(agentFacts(workers, "review").role).toBe("review lead");
    const worker = agentFacts(
      [workerProcess("web-worker", { role: "task_worker" })],
      "web-worker",
    );
    expect(worker.role).toBe("task worker of web");
    expect(worker.delivery).not.toContain("main agent");
    const main = agentFacts(workers, "main");
    expect(main.role).toBe("Claude Code session");
    expect(main.delivery).toContain("as a notice");
  });
});

describe("instructionsChange", () => {
  it("sets, clears or keeps the instructions", () => {
    expect(instructionsChange("Draw first.", " Draw first. ")).toBeNull();
    expect(instructionsChange(null, "  ")).toBeNull();
    expect(instructionsChange("Draw first.", "")).toEqual({
      clear_instructions: true,
    });
    expect(instructionsChange(null, " Use the tokens. ")).toEqual({
      instructions: "Use the tokens.",
    });
  });

  it("holds the daemon's bound", () => {
    expect(
      instructionsSchema.safeParse({ text: "x".repeat(MAX_INSTRUCTIONS) })
        .success,
    ).toBe(true);
    expect(
      instructionsSchema.safeParse({ text: "x".repeat(MAX_INSTRUCTIONS + 1) })
        .success,
    ).toBe(false);
  });
});

describe("parseChatSearch", () => {
  it("keeps a note id and drops anything else", () => {
    expect(parseChatSearch({ note: "n-4" })).toEqual({ note: "n-4" });
    expect(parseChatSearch({ note: "T-4" })).toEqual({});
    expect(parseChatSearch({ note: 4 })).toEqual({});
    expect(parseChatSearch({})).toEqual({});
  });
});

describe("noteDraft", () => {
  it("starts a reply to a note with its mention", () => {
    expect(noteDraft("n-4")).toBe("#n-4 ");
    expect(noteDraft(undefined)).toBe("");
  });
});
