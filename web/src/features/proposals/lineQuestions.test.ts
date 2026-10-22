import { describe, expect, it } from "vitest";
import type { MessageLines } from "@/api/messages";
import { message, recipient, thread } from "@/test/messages";
import {
  proposalDetail,
  proposalSummary,
  proposalVersion,
} from "@/test/proposals";
import {
  NAME_NOTE,
  SINCE_OLD_SIDE_NOTE,
  askTitle,
  lineMenu,
  lineQuestions,
  questionHref,
  questionLabel,
  questionMarks,
  type LineQuestion,
} from "./lineQuestions";
import { resolveView } from "./view";

function lines(fields: Partial<MessageLines> = {}): MessageLines {
  return {
    version: 2,
    path: "a.sql",
    side: "new",
    start_line: 2,
    end_line: 4,
    excerpt: "b\nc\nd",
    excerpt_cut: false,
    ...fields,
  };
}

function proposal(state: "open" | "integrated" = "open") {
  return proposalDetail(proposalSummary("P-4", { current_version: 2, state }), {
    versions: [proposalVersion(1), proposalVersion(2)],
  });
}

function question(fields: Partial<LineQuestion> = {}): LineQuestion {
  return {
    id: "m-5",
    thread: "m-5",
    version: 2,
    range: { path: "a.sql", side: "new", start_line: 2, end_line: 4 },
    to: ["backend/db"],
    answered: false,
    ...fields,
  };
}

describe("lineQuestions", () => {
  it("reads each question about the proposal's lines, oldest first, answered once an agent writes after it", () => {
    const asked = message("m-12", {
      thread: "m-12",
      sender: "human:tanim",
      proposal: "P-4",
      recipients: [recipient("backend/db", { addressed_as: "worker" })],
      lines: lines({ start_line: 7, end_line: 7 }),
    });
    const threads = [
      thread("m-12", {
        messages: [
          asked,
          message("m-13", { thread: "m-12", sender: "human:ana" }),
          message("m-14", { thread: "m-12", sender: "backend" }),
        ],
      }),
      thread("m-5", {
        messages: [
          message("m-5", {
            sender: "human:tanim",
            proposal: "P-4",
            recipients: [recipient("backend/db")],
            lines: lines(),
          }),
          message("m-6", { thread: "m-5", sender: "human:tanim" }),
        ],
      }),
      thread("m-9", {
        messages: [
          message("m-9", { proposal: "P-7", lines: lines() }),
          message("m-10", { thread: "m-9", proposal: "P-4" }),
        ],
      }),
    ];
    expect(lineQuestions(threads, "P-4")).toEqual([
      question(),
      question({
        id: "m-12",
        thread: "m-12",
        range: { path: "a.sql", side: "new", start_line: 7, end_line: 7 },
        answered: true,
      }),
    ]);
  });

  it("an earlier message from the recipient does not answer a question", () => {
    const threads = [
      thread("m-1", {
        messages: [
          message("m-1", { thread: "m-1", sender: "backend/db" }),
          message("m-2", {
            thread: "m-1",
            sender: "human:tanim",
            proposal: "P-4",
            recipients: [recipient("backend/db")],
            lines: lines(),
          }),
        ],
      }),
    ];
    expect(lineQuestions(threads, "P-4").map((q) => q.answered)).toEqual([
      false,
    ]);
  });

  it("any agent's later message answers a question, and fold's notice does not", () => {
    const asked = (id: string) =>
      message(id, {
        thread: id,
        sender: "human:tanim",
        proposal: "P-4",
        recipients: [recipient("backend/db")],
        lines: lines(),
      });
    const threads = [
      thread("m-1", {
        messages: [
          asked("m-1"),
          message("m-2", { thread: "m-1", sender: "fold" }),
          message("m-3", { thread: "m-1", sender: "web-lead" }),
        ],
      }),
      thread("m-4", {
        messages: [
          asked("m-4"),
          message("m-5", { thread: "m-4", sender: "fold" }),
        ],
      }),
    ];
    expect(lineQuestions(threads, "P-4").map((q) => q.answered)).toEqual([
      true,
      false,
    ]);
  });
});

describe("questionMarks", () => {
  const p = proposal();
  const older = question({ id: "m-1", version: 1 });
  const oldSide = question({
    id: "m-2",
    range: { path: "a.sql", side: "old", start_line: 1, end_line: 1 },
  });
  const current = question({ id: "m-3" });

  it("marks the questions about the shown version on either side of its full change", () => {
    expect(
      questionMarks([older, oldSide, current], resolveView(p, {})).map(
        (m) => m.id,
      ),
    ).toEqual(["m-2", "m-3"]);
    expect(
      questionMarks([older, current], resolveView(p, { version: 1 })),
    ).toEqual([{ id: "m-1", range: older.range }]);
  });

  it("marks only the new side of a change since an earlier version", () => {
    expect(
      questionMarks([oldSide, current], resolveView(p, { since: 1 })).map(
        (m) => m.id,
      ),
    ).toEqual(["m-3"]);
  });
});

describe("the marker", () => {
  it("names the question, its recipients, its lines and whether it waits", () => {
    expect(questionLabel(question())).toBe(
      "Question m-5 to backend/db about lines 2–4, waiting for an answer",
    );
    expect(
      questionLabel(
        question({
          answered: true,
          to: ["backend/db", "backend"],
          range: { path: "a.sql", side: "new", start_line: 3, end_line: 3 },
        }),
      ),
    ).toBe("Question m-5 to backend/db, backend about line 3, answered");
  });

  it("links to the question's thread on the Messages screen", () => {
    expect(questionHref("demo", question({ thread: "m-5" }))).toBe(
      "/p/demo/messages?thread=m-5",
    );
  });
});

describe("lineMenu", () => {
  it("offers both actions on the current version of an open proposal", () => {
    const p = proposal();
    expect(lineMenu(p, "tanim", resolveView(p, {}), "old", "a.sql")).toEqual({
      ask: "tanim",
      comment: "tanim",
      notes: [],
    });
  });

  it("still asks on an older version, and says where comments go", () => {
    const p = proposal();
    expect(
      lineMenu(p, "tanim", resolveView(p, { version: 1 }), "new", "a.sql"),
    ).toEqual({
      ask: "tanim",
      comment: null,
      notes: [
        {
          text: "This is v1. Comments go on the current version, v2.",
          link: { label: "Open v2", search: { path: "a.sql" } },
        },
      ],
    });
  });

  it("still asks about a proposal that is not open", () => {
    const p = proposal("integrated");
    expect(lineMenu(p, "tanim", resolveView(p, {}), "new", "a.sql")).toEqual({
      ask: "tanim",
      comment: null,
      notes: [
        {
          text: "P-4 is integrated, so it takes no new comments.",
          link: null,
        },
      ],
    });
  });

  it("offers nothing without a name, or on the old side of a change since an earlier version", () => {
    const p = proposal();
    expect(lineMenu(p, null, resolveView(p, {}), "new", "a.sql")).toEqual({
      ask: null,
      comment: null,
      notes: [{ text: NAME_NOTE, link: null }],
    });
    expect(
      lineMenu(p, "tanim", resolveView(p, { since: 1 }), "old", "a.sql"),
    ).toEqual({
      ask: null,
      comment: null,
      notes: [{ text: SINCE_OLD_SIDE_NOTE, link: null }],
    });
    expect(
      lineMenu(p, "tanim", resolveView(p, { since: 1 }), "new", "a.sql").ask,
    ).toBe("tanim");
  });
});

describe("askTitle", () => {
  it("names the lines, the side and the version", () => {
    expect(
      askTitle({ path: "a.sql", side: "new", start_line: 2, end_line: 4 }, 2),
    ).toBe("Ask the author about lines 2–4 · new side · v2");
    expect(
      askTitle({ path: "a.sql", side: "old", start_line: 9, end_line: 9 }, 1),
    ).toBe("Ask the author about line 9 · old side · v1");
  });
});
