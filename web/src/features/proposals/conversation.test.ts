import { describe, expect, it } from "vitest";
import { add, ctx, del, hunk } from "@/test/diff";
import { message, recipient, thread } from "@/test/messages";
import {
  proposalDetail,
  proposalSummary,
  proposalVersion,
  reviewComment,
} from "@/test/proposals";
import {
  conversationThreads,
  filterConversation,
  forwards,
  laterChange,
  linesChanged,
  rangeLines,
  kindOptions,
  lastLine,
  latestThreads,
  NO_FILTER,
  stateOptions,
  versionAt,
  versionOptions,
  versionText,
  whereText,
  type ConversationThread,
} from "./conversation";

const T0 = 1_760_000_000_000;
const versions = [
  proposalVersion(1, { created_at: T0 }),
  proposalVersion(2, { created_at: T0 + 100 }),
];
const p = proposalDetail(proposalSummary("P-4", { current_version: 2 }), {
  versions,
});

const lines = {
  version: 2,
  path: "web/a.ts",
  side: "new" as const,
  start_line: 2,
  end_line: 3,
  excerpt: "a\nb",
  excerpt_cut: false,
};

const waiting = thread("m-1", {
  last_at: T0 + 300,
  messages: [
    message("m-1", {
      sender: "human:tanim",
      recipients: [recipient("frontend")],
      body: "Why two constants?",
      lines,
      sent_at: T0 + 300,
    }),
  ],
});
const answered = thread("m-2", {
  last_at: T0 + 250,
  messages: [
    message("m-2", {
      sender: "human:tanim",
      recipients: [recipient("frontend")],
      body: "Why a 30-day quarantine?",
      sent_at: T0 + 50,
    }),
    message("m-3", {
      thread: "m-2",
      sender: "frontend",
      recipients: [recipient("human:tanim")],
      body: "The settings say so.\nSee the docs.",
      sent_at: T0 + 250,
    }),
  ],
});
const askingYou = thread("m-4", {
  awaiting_human: true,
  closable: true,
  last_at: T0 + 200,
  messages: [
    message("m-4", {
      sender: "frontend",
      recipients: [recipient("humans", { addressed_as: "humans" })],
      asks_human: true,
      body: "Shorter default?",
      sent_at: T0 + 200,
    }),
  ],
});
const answeredByYou = thread("m-5", {
  last_at: T0 + 20,
  messages: [
    message("m-5", {
      sender: "frontend",
      recipients: [recipient("human:tanim")],
      body: "Which one?",
      sent_at: T0 + 10,
    }),
  ],
});
const agents = thread("m-6", {
  last_at: T0 + 10,
  messages: [
    message("m-6", {
      sender: "review/lead",
      recipients: [
        recipient("frontend", { forwarded_to: "web-lead" }),
        recipient("backend", { forwarded_to: "review/lead" }),
      ],
      body: "Merge main.",
      sent_at: T0 + 10,
    }),
  ],
});
const comments = [
  reviewComment(1, 1, { created_at: T0 + 5, outdated: true }),
  reviewComment(2, 2, {
    created_at: T0 + 150,
    path: "web/b.ts",
    start_line: 7,
    end_line: 7,
  }),
];

function all(): ConversationThread[] {
  return conversationThreads(
    p,
    [waiting, answered, askingYou, answeredByYou, agents, thread("m-9")],
    comments,
  );
}

function summary(threads: readonly ConversationThread[]) {
  return threads.map((t) => [t.key, t.kind, t.state, t.stateText, t.version]);
}

describe("conversationThreads", () => {
  it("lists what waits first, each group newest first, with its kind, state and version", () => {
    expect(summary(all())).toEqual([
      ["m-4", "to_you", "needs_answer", "Needs your answer", 2],
      ["m-1", "to_author", "waits_author", "Waits for the author", 2],
      ["rc-2", "review", "open", "Open", 2],
      ["m-2", "to_author", "closed", "Answered", 1],
      ["m-5", "to_you", "closed", "Answered", 1],
      ["m-6", "agent", "closed", "For information", 1],
      ["rc-1", "review", "closed", "Earlier version", 1],
    ]);
    const first = all().find((t) => t.key === "m-1");
    expect(first?.waitsFor).toEqual(["frontend"]);
    expect(first?.lines).toEqual({
      path: "web/a.ts",
      side: "new",
      start_line: 2,
      end_line: 3,
    });
  });

  it("closes every review comment of a proposal that is not open", () => {
    const done = {
      ...p,
      summary: { ...p.summary, state: "integrated" as const },
    };
    expect(
      summary(conversationThreads(done, [], comments)).map((t) => t[3]),
    ).toEqual(["Closed", "Closed"]);
  });
});

describe("filters", () => {
  it("filters by state, kind and version together", () => {
    const threads = all();
    expect(
      filterConversation(threads, { ...NO_FILTER, state: "closed" }).map(
        (t) => t.key,
      ),
    ).toEqual(["m-2", "m-5", "m-6", "rc-1"]);
    expect(
      filterConversation(threads, {
        state: "closed",
        kind: "to_you",
        version: 1,
      }).map((t) => t.key),
    ).toEqual(["m-5"]);
    expect(
      filterConversation(threads, { ...NO_FILTER, version: 2 }).map(
        (t) => t.key,
      ),
    ).toEqual(["m-4", "m-1", "rc-2"]);
  });

  it("counts each option with the other filters as they are", () => {
    const threads = all();
    const kind = { ...NO_FILTER, kind: "to_author" as const };
    expect(
      stateOptions(threads, kind).map((o) => [o.label, o.count, o.selected]),
    ).toEqual([
      ["All", 2, true],
      ["Needs your answer", 0, false],
      ["Waits for the author", 1, false],
      ["Open", 0, false],
      ["Closed", 1, false],
    ]);
    expect(
      kindOptions(threads, { ...NO_FILTER, state: "closed" }).map((o) => [
        o.label,
        o.count,
        o.selected,
      ]),
    ).toEqual([
      ["Questions to the author", 1, false],
      ["Questions to you", 1, false],
      ["Review comments", 1, false],
      ["Agent messages", 1, false],
    ]);
    expect(
      versionOptions(threads, NO_FILTER, 2).map((o) => [
        o.label,
        o.count,
        o.selected,
      ]),
    ).toEqual([
      ["All versions", 7, true],
      ["v2 (current)", 3, false],
      ["v1", 4, false],
    ]);
  });
});

describe("words", () => {
  it("says where a thread is and which version it is on", () => {
    const threads = all();
    const byKey = (key: string) => {
      const found = threads.find((t) => t.key === key);
      if (!found) throw new Error(key);
      return found;
    };
    expect(whereText(byKey("m-1"))).toBe("web/a.ts · lines 2–3");
    expect(whereText(byKey("m-2"))).toBe("whole proposal");
    expect(versionText(byKey("m-1"), 2, false)).toBe("v2");
    expect(versionText(byKey("rc-1"), 2, true)).toBe("on v1 · outdated");
    expect(versionText(byKey("rc-1"), 2, false)).toBe("on v1");
    expect(
      versionText({ ...byKey("m-1"), version: null }, 2, false),
    ).toBeNull();
    expect(lastLine(byKey("m-2"))).toEqual({
      author: "frontend",
      text: "The settings say so.",
    });
    expect(lastLine(byKey("rc-2"))).toEqual({
      author: "review/quality",
      text: "comment 2",
    });
  });

  it("lists the newest threads and the forwards to a lead", () => {
    expect(latestThreads(all()).map((t) => t.key)).toEqual([
      "m-1",
      "m-2",
      "m-4",
    ]);
    const sent = agents.messages[0];
    if (!sent) throw new Error("no message");
    expect(forwards(sent)).toEqual([{ from: "frontend", to: "web-lead" }]);
  });

  it("finds the version current at a time", () => {
    expect(versionAt(versions, T0 + 150)).toBe(2);
    expect(versionAt(versions, T0 - 1)).toBe(1);
    expect(versionAt([], T0)).toBeNull();
  });
});

describe("lines a later version changed", () => {
  it("reads the change since the thread's version for its new-side lines", () => {
    const threads = all();
    const rc1 = threads.find((t) => t.key === "rc-1");
    const m1 = threads.find((t) => t.key === "m-1");
    if (!rc1 || !m1) throw new Error("missing");
    expect(laterChange(rc1, 2)).toEqual({ since: 1, path: "a.sql" });
    expect(laterChange(m1, 2)).toBeNull();
    expect(
      laterChange(
        { ...rc1, lines: rc1.lines && { ...rc1.lines, side: "old" } },
        2,
      ),
    ).toBeNull();
    expect(laterChange({ ...rc1, lines: null }, 2)).toBeNull();
  });

  it("finds a removed or rewritten line inside the range", () => {
    const hunks = [hunk(1, 1, [ctx("a"), del("b"), add("B"), ctx("c")])];
    const range = {
      path: "a",
      side: "new" as const,
      start_line: 2,
      end_line: 2,
    };
    expect(linesChanged(hunks, range)).toBe(true);
    expect(linesChanged(hunks, { ...range, start_line: 3, end_line: 4 })).toBe(
      false,
    );
    expect(linesChanged([], range)).toBe(false);
  });
});

describe("rangeLines", () => {
  it("takes the range's lines on its side from the hunks", () => {
    const hunks = [hunk(1, 1, [ctx("a"), del("b"), add("B"), ctx("c")])];
    const range = {
      path: "a",
      side: "new" as const,
      start_line: 2,
      end_line: 3,
    };
    expect(rangeLines(hunks, range)).toEqual([
      { n: 2, text: "B", kind: "added" },
      { n: 3, text: "c", kind: "context" },
    ]);
    expect(
      rangeLines(hunks, { ...range, side: "old", start_line: 2, end_line: 2 }),
    ).toEqual([{ n: 2, text: "b", kind: "removed" }]);
    expect(rangeLines(hunks, { ...range, start_line: 9, end_line: 9 })).toEqual(
      [],
    );
  });
});
