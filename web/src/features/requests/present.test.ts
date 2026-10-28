import { describe, expect, it } from "vitest";
import {
  T0,
  card,
  requestComment,
  requestDetail,
  requestDraft,
  requestSummary,
} from "@/test/requests";
import {
  KIND_LABELS,
  STATE_LABELS,
  authorLine,
  commentsSummary,
  createdTasks,
  drafterLine,
  edgeLines,
  edgesSummary,
  formatSize,
  headingOf,
  joinIds,
  kindOf,
  listSummary,
  metaLine as metaLineSegments,
  pageTitle,
  rowSubLine,
  shownDraft,
  stateChip,
  statusHeading as statusHeadingSegments,
  threadSummary,
  versionText,
} from "./present";
import { segmentsText } from "@/people/segments";

const metaLine = (...args: Parameters<typeof metaLineSegments>) =>
  segmentsText(metaLineSegments(...args));
const statusHeading = (...args: Parameters<typeof statusHeadingSegments>) =>
  segmentsText(statusHeadingSegments(...args));

const HOUR = 3_600_000;

describe("state and kind", () => {
  it("names every state and kind", () => {
    expect(STATE_LABELS.draft_ready).toBe("Draft ready");
    expect(KIND_LABELS.unsorted).toBe("Unsorted");
  });

  it("marks in gold only a draft that waits for the signed-in requester", () => {
    const ready = requestSummary("RQ-4", { state: "draft_ready" });
    expect(stateChip(ready, "tanim")).toEqual({
      label: "Draft ready · waits for you",
      tone: "warn",
    });
    expect(stateChip(ready, "rafeed")).toEqual({
      label: "Draft ready",
      tone: "neutral",
    });
    expect(
      stateChip(requestSummary("RQ-4", { state: "approved" }), "tanim"),
    ).toEqual({ label: "Approved", tone: "ok" });
    expect(stateChip(requestSummary("RQ-4"), null)).toEqual({
      label: "Open",
      tone: "neutral",
    });
  });

  it("shows a request no orchestrator sorted as unsorted", () => {
    expect(kindOf(requestSummary("RQ-1"))).toBe("unsorted");
    expect(kindOf(requestSummary("RQ-1", { kind: "bug" }))).toBe("bug");
  });

  it("uses the excerpt in a row until the request has a title", () => {
    expect(headingOf(requestSummary("RQ-7"))).toEqual({
      text: "Message of RQ-7",
      excerpt: true,
    });
    expect(
      headingOf(requestSummary("RQ-4", { title: "Report export" })),
    ).toEqual({
      text: "Report export",
      excerpt: false,
    });
  });

  it("names an untitled request by its id on its page", () => {
    expect(pageTitle(requestSummary("RQ-7"))).toBe("Request RQ-7");
    expect(pageTitle(requestSummary("RQ-4", { title: "Report export" }))).toBe(
      "Report export",
    );
  });

  it("writes the id and kind under a row's title", () => {
    expect(rowSubLine(requestSummary("RQ-7"))).toBe(
      "RQ-7 · Unsorted · no title yet",
    );
    expect(
      rowSubLine(requestSummary("RQ-4", { kind: "bug", title: "x" })),
    ).toBe("RQ-4 · Bug");
    expect(rowSubLine(requestSummary("RQ-7"), "2 d")).toBe(
      "RQ-7 · Unsorted · 2 d",
    );
  });
});

describe("formatSize and joinIds", () => {
  it("writes binary units", () => {
    expect(formatSize(900)).toBe("900 B");
    expect(formatSize(18 * 1024 + 100)).toBe("18 KiB");
    expect(formatSize(14.2 * 1024 * 1024)).toBe("14.2 MiB");
  });

  it("joins ids as a sentence", () => {
    expect(joinIds([])).toBe("");
    expect(joinIds(["T-1"])).toBe("T-1");
    expect(joinIds(["T-1", "T-2"])).toBe("T-1 and T-2");
    expect(joinIds(["T-1", "T-2", "T-3"])).toBe("T-1, T-2 and T-3");
  });
});

describe("metaLine", () => {
  const now = T0 + 3 * HOUR;
  it("says who asked and when, and that a title is missing", () => {
    expect(metaLine(requestDetail(requestSummary("RQ-4")), now)).toBe(
      "Requested by @tanim 3 h ago · No title yet",
    );
    expect(
      metaLine(
        requestDetail(requestSummary("RQ-4", { title: "Report export" })),
        now,
      ),
    ).toBe("Requested by @tanim 3 h ago");
  });
});

describe("statusHeading", () => {
  const now = T0 + 3 * HOUR;
  const line = (
    fields: Parameters<typeof requestSummary>[1],
    tasks: string[] = [],
    human: string | null = "tanim",
  ) =>
    statusHeading(
      requestDetail(requestSummary("RQ-4", fields), { tasks }),
      human,
      now,
    );

  it("says who acts next in each state", () => {
    expect(line({})).toBe("No orchestrator has claimed it yet");
    expect(line({ state: "drafting", drafter: "backend" })).toBe(
      "@backend is drafting the cards",
    );
    expect(
      line({ state: "drafting", drafter: "backend", latest_version: 1 }),
    ).toBe("@backend is drafting the version after v1");
    expect(line({ state: "drafting" })).toBe(
      "An orchestrator is drafting the cards",
    );
    expect(line({ state: "drafting", latest_version: 2 })).toBe(
      "An orchestrator is drafting the version after v2",
    );
    expect(
      line({ state: "draft_ready", drafter: "backend", latest_version: 2 }),
    ).toBe("v2 waits for your decision");
    expect(
      line(
        { state: "draft_ready", drafter: "backend", latest_version: 2 },
        [],
        "rafeed",
      ),
    ).toBe("v2 waits for @tanim");
    expect(
      line({
        state: "changes_requested",
        drafter: "backend",
        latest_version: 1,
      }),
    ).toBe("v1 went back to @backend");
    expect(line({ state: "closed", updated_at: now - 60_000 })).toBe(
      "Closed 1 min ago",
    );
  });

  it("names the tasks an approval created", () => {
    const approved = {
      state: "approved" as const,
      latest_version: 2,
      updated_at: T0,
    };
    expect(line(approved, ["T-12", "T-13"])).toMatch(
      /^@tanim approved v2 at .+ · T-12 and T-13 were created$/,
    );
    expect(line(approved, ["T-12"])).toMatch(/T-12 was created$/);
    expect(line(approved)).toMatch(/approved v2 at [^·]+$/);
  });
});

describe("draft lines", () => {
  it("counts versions and marks the approved one", () => {
    expect(versionText(1, 2, "draft_ready")).toBe("Version 1 of 2");
    expect(versionText(2, 2, "approved")).toBe("Version 2 of 2 · approved");
    expect(versionText(1, 2, "approved")).toBe("Version 1 of 2");
  });

  it("writes the author line with the note", () => {
    const now = T0 + HOUR;
    expect(authorLine(requestDraft(2), now)).toMatch(/^wrote v2 at \d\d:\d\d$/);
    expect(authorLine(requestDraft(2, { note: " split it " }), now)).toMatch(
      / · Note: split it$/,
    );
  });

  it("reads each edge as dependent needs prerequisite, by title", () => {
    expect(
      edgeLines(
        [
          { dependent: "c2", prerequisite: "c1" },
          { dependent: "c3", prerequisite: "c9" },
        ],
        [card("c1", "Query"), card("c2", "CSV"), card("c3", "PDF")],
      ),
    ).toEqual([
      { dependent: "c2", prerequisite: "c1", text: "CSV needs Query" },
      { dependent: "c3", prerequisite: "c9", text: "PDF needs c9" },
    ]);
    expect(edgesSummary(0)).toBe("No edges · the cards can start in any order");
    expect(edgesSummary(1)).toBe(
      "1 edge · a card waits until the cards it needs are done",
    );
    expect(edgesSummary(2)).toMatch(/^2 edges/);
  });

  it("counts comments per version", () => {
    const drafts = [requestDraft(1), requestDraft(2)];
    expect(commentsSummary([requestComment(1, "split")], drafts)).toBe(
      "1 on v1, none on v2",
    );
    expect(commentsSummary([], [])).toBe("none yet");
  });

  it("shows the chosen version, else the latest", () => {
    const drafts = [requestDraft(1), requestDraft(2)];
    expect(shownDraft(drafts, 1)?.version).toBe(1);
    expect(shownDraft(drafts, null)?.version).toBe(2);
    expect(shownDraft(drafts, 9)?.version).toBe(2);
    expect(shownDraft([], null)).toBeNull();
  });

  it("names the drafter's latest version", () => {
    const drafts = [
      requestDraft(1),
      requestDraft(2, { author: "human:tanim" }),
    ];
    expect(drafterLine(drafts, "backend", T0)).toMatch(/^Draft v1 submitted /);
    expect(drafterLine([], "backend", T0)).toBe("No draft submitted yet");
  });
});

describe("summaries", () => {
  it("counts thread messages", () => {
    expect(threadSummary(0, "RQ-4")).toBe("No messages yet about RQ-4");
    expect(threadSummary(1, "RQ-4")).toBe("1 message in the thread of RQ-4");
    expect(threadSummary(2, "RQ-4")).toBe("2 messages in the thread of RQ-4");
  });

  it("counts requests and the drafts that wait for a human", () => {
    expect(listSummary([])).toEqual({ total: "0 requests", waiting: null });
    expect(listSummary([requestSummary("RQ-1")]).total).toBe("1 request");
    expect(
      listSummary([
        requestSummary("RQ-1", { state: "draft_ready" }),
        requestSummary("RQ-2", { state: "draft_ready", requester: "backend" }),
        requestSummary("RQ-3"),
      ]),
    ).toEqual({ total: "3 requests", waiting: "1 needs a human" });
    expect(
      listSummary([
        requestSummary("RQ-1", { state: "draft_ready" }),
        requestSummary("RQ-2", { state: "draft_ready" }),
      ]),
    ).toEqual({ total: "2 requests", waiting: "2 need a human" });
  });
});

describe("createdTasks", () => {
  it("pairs each task with its card and its column on the board", () => {
    const detail = requestDetail(
      requestSummary("RQ-4", { state: "approved" }),
      {
        drafts: [
          requestDraft(1, { cards: [card("c1", "Query"), card("c2", "CSV")] }),
        ],
        tasks: ["T-12", "T-13"],
      },
    );
    expect(
      createdTasks(detail, [
        { task: "T-12", title: "Query on board", status: "todo" },
      ]),
    ).toEqual([
      { task: "T-12", key: "c1", title: "Query on board", status: "todo" },
      { task: "T-13", key: "c2", title: "CSV", status: null },
    ]);
    expect(
      createdTasks(
        requestDetail(requestSummary("RQ-5"), { tasks: ["T-1"] }),
        undefined,
      ),
    ).toEqual([{ task: "T-1", key: null, title: "", status: null }]);
  });
});
