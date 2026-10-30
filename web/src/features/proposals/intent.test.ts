import { describe, expect, it } from "vitest";
import {
  intent,
  proposalDetail,
  proposalSummary,
  proposalVersion,
  review,
} from "@/test/proposals";
import { historyEntries } from "./history";
import {
  NO_INTENT,
  checkedBy,
  coverage,
  intentByline,
  intentCard,
  intentInHistory,
  intentSections,
  partOf,
  partSize,
  reviewerSummary,
  versionIntent,
} from "./intent";

const TABLE = intent("Adds the table.", [
  ["Table", "Adds the table.", ["db/0001.sql"]],
]);

function entries(
  versions: ReturnType<typeof proposalVersion>[],
  reviews: ReturnType<typeof review>[] = [],
) {
  const p = proposalDetail(
    proposalSummary("P-4", { current_version: versions.length }),
    { versions, reviews },
  );
  return historyEntries(p, new Map());
}

describe("the intent card", () => {
  it("shows the current version's intent and who wrote it", () => {
    const card = intentCard(
      entries([
        proposalVersion(1, { intent: TABLE }),
        proposalVersion(2, {
          intent: { ...TABLE, summary: "Adds the table and its index." },
          created_by: "review/quality",
          reviewer_fix: true,
        }),
      ]),
    );
    expect(card).toMatchObject({
      version: 2,
      intent: { summary: "Adds the table and its index." },
      by: "review/quality",
      origin: "review_fix",
      summary: null,
      keptFrom: null,
    });
    expect(card === null ? null : intentByline(card)).toEqual({
      kind: "written",
      words: "written in a review fix by",
      by: "review/quality",
    });
  });

  it("says a human edit kept the intent of the version before, and names nobody without an intent", () => {
    const kept = intentCard(
      entries([
        proposalVersion(1, { intent: TABLE }),
        proposalVersion(2, {
          intent: { ...TABLE },
          created_by: "human:tanim",
          human_edit: true,
        }),
      ]),
    );
    expect(kept?.keptFrom).toBe(1);
    expect(kept === null ? null : intentByline(kept)).toEqual({
      kind: "kept",
      from: 1,
    });
    const own = intentCard(
      entries([
        proposalVersion(1, { intent: TABLE }),
        proposalVersion(2, {
          intent: { ...TABLE, summary: "Adds the table again." },
          created_by: "human:tanim",
          human_edit: true,
        }),
      ]),
    );
    expect(own === null ? null : intentByline(own)).toEqual({
      kind: "written",
      words: "written in a human edit by",
      by: "human:tanim",
    });
    const none = intentCard(entries([proposalVersion(1), proposalVersion(2)]));
    expect(none?.keptFrom).toBeNull();
    expect(none === null ? "card" : intentByline(none)).toBeNull();
    const fromNone = intentCard(
      entries([proposalVersion(1), proposalVersion(2, { intent: TABLE })]),
    );
    expect(fromNone?.keptFrom).toBeNull();
  });

  it("has no intent for a version made before intents, and no card without a version", () => {
    expect(intentCard(entries([proposalVersion(1)]))?.intent).toBeNull();
    expect(intentCard([])).toBeNull();
    expect(NO_INTENT).toBe("This version has no intent.");
  });

  it("shows an intent in the history only on a version the Intent card does not show", () => {
    const list = entries([proposalVersion(1), proposalVersion(2)]);
    const card = intentCard(list);
    expect(
      list.map((entry) => [
        entry.version.version,
        intentInHistory(entry, card),
      ]),
    ).toEqual([
      [2, false],
      [1, true],
    ]);
    expect(intentInHistory({ version: proposalVersion(2) }, null)).toBe(true);
  });

  it("takes the newest verdict as the reviewer's summary, and a comment only without one", () => {
    const withVerdict = intentCard(
      entries(
        [proposalVersion(1)],
        [
          review(1, "review/a", "approve", {
            body: "Matches the intent.",
            created_at: 1,
          }),
          review(1, "review/a", "comment", { body: "a note", created_at: 2 }),
        ],
      ),
    );
    expect(withVerdict?.summary?.review.verdict).toBe("approve");
    expect(withVerdict === null ? null : checkedBy(withVerdict)).toBe(
      "review/a",
    );
    expect(withVerdict?.summary?.excerpt.text).toBe("Matches the intent.");
    const onlyComment = reviewerSummary(
      entries(
        [proposalVersion(1)],
        [review(1, "review/a", "comment", { body: "a note" })],
      )[0]?.reviews ?? [],
    );
    expect(onlyComment?.review.verdict).toBe("comment");
    const commented = intentCard(
      entries(
        [proposalVersion(1)],
        [review(1, "review/a", "comment", { body: "a note" })],
      ),
    );
    expect(commented === null ? "card" : checkedBy(commented)).toBeNull();
    expect(reviewerSummary([])).toBeNull();
  });

  it("reads an intent with a blank summary as none", () => {
    expect(versionIntent({ intent: { ...TABLE, summary: " \n" } })).toBeNull();
    expect(versionIntent({ intent: null })).toBeNull();
    expect(versionIntent({ intent: TABLE })).toBe(TABLE);
  });
});

describe("the sections of an intent", () => {
  it("shows only the contract kinds, decisions and not-changed entries it has, in the reader's order", () => {
    const full = intent("s", [["P", "d", ["a"]]], {
      contracts: {
        routes: ["POST /cases"],
        database: [],
        rights: [],
        config: [],
        errors: ["case_open"],
        events: [],
        commands: [],
      },
      decisions: ["One case per kind."],
    });
    expect(intentSections(full)).toEqual({
      contracts: [
        { kind: "routes", label: "Routes", entries: ["POST /cases"] },
        { kind: "errors", label: "Errors", entries: ["case_open"] },
      ],
      decisions: ["One case per kind."],
      notChanged: [],
    });
    expect(intentSections(TABLE)).toEqual({
      contracts: [],
      decisions: [],
      notChanged: [],
    });
  });

  it("says that every changed file belongs to a part, from the version's diff", () => {
    const many = intent("s", [
      ["Routes", "d", ["a", "b"]],
      ["Tests", "d", ["c"]],
    ]);
    expect(coverage(many, ["a", "b", "c"])).toEqual({
      text: "All 3 changed files belong to a part below.",
      outside: 0,
    });
    expect(
      coverage(intent("s", [["P", "d", ["a", "b", "c"]]]), ["a", "b", "c"])
        .text,
    ).toBe("All 3 changed files belong to the part below.");
    expect(
      coverage(intent("s", [["P", "d", ["a", "b"]]]), ["a", "b"]).text,
    ).toBe("Both changed files belong to the part below.");
    expect(coverage(TABLE, ["db/0001.sql"]).text).toBe(
      "The changed file belongs to the part below.",
    );
    expect(coverage(intent("s", [["P", "d", []]]), []).text).toBe(
      "This version changes no file.",
    );
  });

  it("counts the changed files that no part names", () => {
    expect(coverage(TABLE, ["db/0001.sql", "src/a.ts"])).toEqual({
      text: "1 changed file belongs to no part.",
      outside: 1,
    });
    expect(coverage(TABLE, ["src/a.ts", "src/b.ts"])).toEqual({
      text: "2 changed files belong to no part.",
      outside: 2,
    });
  });

  it("finds a file's part and sizes each part from the diff", () => {
    const two = intent("s", [
      ["Routes", "Opens a case.", ["src/routes.rs", "src/schemas.rs"]],
      ["Tests", "Covers it.", ["tests/cases.rs"]],
    ]);
    expect(partOf(two, "tests/cases.rs")?.name).toBe("Tests");
    expect(partOf(two, "README.md")).toBeNull();
    expect(partOf(null, "tests/cases.rs")).toBeNull();
    const files = [
      { path: "src/routes.rs", added_lines: 540, removed_lines: 0 },
      { path: "src/schemas.rs", added_lines: 72, removed_lines: 12 },
    ];
    const [routes, tests] = two.parts;
    if (routes === undefined || tests === undefined) throw new Error("parts");
    expect(partSize(routes, files)).toBe("2 files · +612 −12");
    expect(partSize(tests, files)).toBe("1 file");
    expect(
      partSize(tests, [
        { path: "tests/cases.rs", added_lines: 0, removed_lines: 4 },
      ]),
    ).toBe("1 file · −4");
  });
});
