import { describe, expect, it } from "vitest";
import type { ReviewComment } from "@/api/proposals";
import { reviewComment } from "@/test/proposals";
import {
  buildThreads,
  commentNote,
  composerTitle,
  diffThreads,
  groupByFile,
  linesText,
  unplacedThreads,
} from "./threads";

function comment(
  id: number,
  version: number,
  fields: Partial<ReviewComment> = {},
): ReviewComment {
  return {
    id: `rc-${String(id)}`,
    version,
    author: "review/quality",
    path: "a.sql",
    side: "new",
    start_line: 2,
    end_line: 4,
    reply_to: null,
    body: `comment ${String(id)}`,
    created_at: id,
    verdict: "comment",
    outdated: false,
    batch_comment: null,
    ...fields,
  };
}

describe("buildThreads", () => {
  it("groups starting comments on the same range of the same version into one thread", () => {
    const threads = buildThreads(
      [
        comment(1, 1),
        comment(2, 1),
        comment(3, 1, { start_line: 5, end_line: 5 }),
      ],
      1,
    );
    expect(threads.map((t) => t.comments.map((c) => c.comment.id))).toEqual([
      ["rc-1", "rc-2"],
      ["rc-3"],
    ]);
  });

  it("keeps the same range on two versions apart and the same lines on two sides apart", () => {
    const threads = buildThreads(
      [comment(1, 1), comment(2, 2), comment(3, 2, { side: "old" })],
      2,
    );
    expect(threads.map((t) => t.id)).toEqual(["rc-1", "rc-2", "rc-3"]);
  });

  it("nests replies under the comment they answer, in time order, whatever version they were written on", () => {
    const threads = buildThreads(
      [
        comment(1, 1),
        comment(4, 2, {
          reply_to: "rc-2",
          path: null,
          side: null,
          start_line: null,
          end_line: null,
        }),
        comment(2, 2, { reply_to: "rc-1" }),
        comment(3, 2, { reply_to: "rc-1" }),
      ],
      2,
    );
    expect(threads).toHaveLength(1);
    expect(threads[0]?.comments.map((c) => [c.comment.id, c.reply])).toEqual([
      ["rc-1", false],
      ["rc-2", true],
      ["rc-4", true],
      ["rc-3", true],
    ]);
  });

  it("marks a thread from an earlier version outdated on a later version, and not on its own", () => {
    const comments = [
      comment(1, 1),
      comment(2, 2, { start_line: 7, end_line: 7 }),
    ];
    expect(buildThreads(comments, 2).map((t) => [t.id, t.outdated])).toEqual([
      ["rc-1", true],
      ["rc-2", false],
    ]);
    expect(buildThreads(comments, 1).map((t) => [t.id, t.outdated])).toEqual([
      ["rc-1", false],
    ]);
  });

  it("starts a thread at a reply whose parent is unknown and stops at a reply loop", () => {
    const threads = buildThreads(
      [
        comment(1, 1, { reply_to: "rc-9" }),
        comment(2, 1, { reply_to: "rc-3", start_line: 8, end_line: 8 }),
        comment(3, 1, { reply_to: "rc-2", start_line: 8, end_line: 8 }),
      ],
      1,
    );
    expect(threads.map((t) => t.id)).toEqual(["rc-1", "rc-2"]);
    expect(threads[1]?.comments.map((c) => c.comment.id)).toEqual([
      "rc-2",
      "rc-3",
    ]);
  });

  it("gives a comment on the whole version no range", () => {
    const [thread] = buildThreads(
      [
        comment(1, 1, {
          path: null,
          side: null,
          start_line: null,
          end_line: null,
        }),
      ],
      1,
    );
    expect(thread?.range).toBeNull();
    expect(thread ? unplacedThreads([thread], ["a.sql"]) : []).toHaveLength(1);
  });
});

describe("placing threads", () => {
  const threads = buildThreads(
    [
      comment(1, 1, { path: "b.rs", start_line: 9, end_line: 9 }),
      comment(2, 1, { path: "b.rs", start_line: 3, end_line: 3 }),
      comment(5, 1, { path: "b.rs", start_line: 3, end_line: 6 }),
      comment(3, 1),
      comment(4, 1, {
        path: null,
        side: null,
        start_line: null,
        end_line: null,
      }),
    ],
    1,
  );

  it("groups line threads by file in path order and line order", () => {
    expect(
      groupByFile(threads).map((f) => [
        f.path,
        f.threads.map((t) => t.thread.id),
      ]),
    ).toEqual([
      ["a.sql", ["rc-3"]],
      ["b.rs", ["rc-2", "rc-5", "rc-1"]],
    ]);
  });

  it("gives the diff viewer an anchor for every line thread", () => {
    expect(diffThreads(threads).map((t) => [t.id, t.range.start_line])).toEqual(
      [
        ["rc-3", 2],
        ["rc-2", 3],
        ["rc-5", 3],
        ["rc-1", 9],
      ],
    );
  });

  it("lists the threads the diff cannot place", () => {
    expect(unplacedThreads(threads, ["a.sql"]).map((t) => t.id)).toEqual([
      "rc-1",
      "rc-2",
      "rc-5",
      "rc-4",
    ]);
  });
});

describe("thread text", () => {
  it("names one line or a run of lines", () => {
    expect(
      linesText({ path: "a", side: "new", start_line: 10, end_line: 10 }),
    ).toBe("line 10");
    expect(
      linesText({ path: "a", side: "old", start_line: 2, end_line: 4 }),
    ).toBe("lines 2–4");
  });

  it("notes the version, lines and outdated mark of a first comment and the version of a reply", () => {
    const [thread] = buildThreads(
      [
        comment(1, 1, { start_line: 10, end_line: 10 }),
        comment(2, 2, { reply_to: "rc-1" }),
      ],
      2,
    );
    if (!thread) throw new Error("no thread");
    expect(thread.comments.map((c) => commentNote(thread, c))).toEqual([
      "on v1 · line 10 · outdated",
      "reply · v2",
    ]);
    const [general] = buildThreads(
      [
        comment(3, 2, {
          path: null,
          side: null,
          start_line: null,
          end_line: null,
        }),
      ],
      2,
    );
    if (!general) throw new Error("no thread");
    expect(
      commentNote(
        general,
        general.comments[0] ?? { comment: comment(3, 2), reply: false },
      ),
    ).toBe("on v2");
  });

  it("titles the composer with the range, side and human", () => {
    expect(
      composerTitle(
        { path: "a", side: "new", start_line: 2, end_line: 4 },
        "tanim",
      ),
    ).toBe("Comment on lines 2–4 · new side · tanim");
  });
});

describe("threads from the daemon's comments of several versions", () => {
  // As the comments route answers for P-4 at v3: `outdated` is relative to the current version, the threads to the version on screen.
  const payload: ReviewComment[] = [
    reviewComment(1, 1, {
      author: "review/security",
      verdict: "request_changes",
      start_line: 10,
      end_line: 10,
      outdated: true,
    }),
    reviewComment(2, 2, {
      author: "backend/db",
      reply_to: "rc-1",
      start_line: null,
      end_line: null,
      path: null,
      side: null,
      outdated: true,
    }),
    reviewComment(3, 2, { start_line: 3, end_line: 5, outdated: true }),
    reviewComment(4, 3, {
      author: "human:tanim",
      start_line: 3,
      end_line: 5,
      outdated: false,
    }),
  ];

  it("marks threads by the version numbers against the version on screen, not by the daemon's flag", () => {
    const onV2 = buildThreads(payload, 2);
    expect(onV2.map((t) => [t.id, t.version, t.outdated])).toEqual([
      ["rc-1", 1, true],
      ["rc-3", 2, false],
    ]);
    const onV3 = buildThreads(payload, 3);
    expect(onV3.map((t) => [t.id, t.version, t.outdated])).toEqual([
      ["rc-1", 1, true],
      ["rc-3", 2, true],
      ["rc-4", 3, false],
    ]);
    const onV1 = buildThreads(payload, 1);
    expect(onV1.map((t) => [t.id, t.outdated])).toEqual([["rc-1", false]]);
  });

  it("keeps a reply from a later version under the comment it answers", () => {
    const [first] = buildThreads(payload, 3);
    if (!first) throw new Error("no thread");
    expect(first.comments.map((c) => [c.comment.id, c.reply])).toEqual([
      ["rc-1", false],
      ["rc-2", true],
    ]);
    const [start, reply] = first.comments;
    if (!start || !reply) throw new Error("no comments");
    expect(commentNote(first, start)).toBe("on v1 · line 10 · outdated");
    expect(commentNote(first, reply)).toBe("reply · v2");
  });
});
