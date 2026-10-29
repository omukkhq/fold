import { describe, expect, it } from "vitest";
import { intent } from "@/test/proposals";
import {
  SECTIONS,
  addPart,
  changePart,
  changeSection,
  draftOf,
  draftProblem,
  intentOf,
  lines,
  removePart,
  sameIntent,
} from "./intentDraft";

const FULL = intent(
  "Opens a case.",
  [
    ["Routes", "Opens a case.", ["src/routes.rs", "src/schemas.rs"]],
    ["Tests", "Covers it.", ["tests/routes.rs"]],
  ],
  {
    contracts: {
      routes: ["POST /cases"],
      database: [],
      rights: [],
      config: [],
      errors: ["case_open", "case_closed"],
      events: [],
      commands: [],
    },
    decisions: ["One case per kind."],
    not_changed: ["Sign-in."],
  },
);

describe("the intent form", () => {
  it("starts from the edited version's intent and sends it back unchanged", () => {
    const draft = draftOf(FULL, []);
    expect(draft.parts[0]?.files).toBe("src/routes.rs\nsrc/schemas.rs");
    expect(draft.sections.errors).toBe("case_open\ncase_closed");
    expect(draft.sections.not_changed).toBe("Sign-in.");
    expect(intentOf(draft)).toEqual(FULL);
    expect(sameIntent(intentOf(draft), FULL)).toBe(true);
    expect(SECTIONS.map((section) => section.key)).toEqual([
      "routes",
      "database",
      "rights",
      "config",
      "errors",
      "events",
      "commands",
      "decisions",
      "not_changed",
    ]);
  });

  it("starts a version without an intent with one part of the changed files", () => {
    const draft = draftOf(null, ["a.rs", "b.rs"]);
    expect(draft.summary).toBe("");
    expect(draft.parts).toEqual([{ name: "", does: "", files: "a.rs\nb.rs" }]);
    expect(intentOf(draft).contracts.routes).toEqual([]);
  });

  it("trims every text and drops blank lines", () => {
    expect(lines(" a \n\n  b\n ")).toEqual(["a", "b"]);
    const draft = changeSection(
      changePart(draftOf(FULL, []), 1, {
        name: "  Tests ",
        files: "\n tests/routes.rs \n",
      }),
      "decisions",
      " One case per kind. \n\n",
    );
    expect(intentOf(draft)).toEqual(FULL);
  });

  it("adds and removes parts", () => {
    const more = addPart(draftOf(FULL, []));
    expect(more.parts).toHaveLength(3);
    expect(more.parts[2]).toEqual({ name: "", does: "", files: "" });
    expect(removePart(more, 0).parts.map((part) => part.name)).toEqual([
      "Tests",
      "",
    ]);
  });

  it("names what keeps Save off", () => {
    const start = draftOf(FULL, []);
    const path = "src/routes.rs";
    const other = { name: "Other", does: "d", files: "" };
    expect(draftProblem(start, path)).toBeNull();
    const cases: [ReturnType<typeof draftOf>, string][] = [
      [
        { ...start, summary: "  " },
        "Write the summary: how the change works in the code.",
      ],
      [
        { ...start, summary: "x".repeat(1201) },
        "The summary can hold at most 1200 characters; it has 1201.",
      ],
      [{ ...start, parts: [] }, "Add a part."],
      [
        { ...start, parts: Array.from({ length: 13 }, () => other) },
        "An intent has at most 12 parts.",
      ],
      [changePart(start, 1, { name: " " }), "Part 2 needs a name."],
      [
        changePart(start, 1, { name: "n".repeat(81) }),
        "Part 2: a name has at most 80 characters.",
      ],
      [changePart(start, 1, { name: "Routes" }), "Two parts are named Routes."],
      [
        changePart(start, 0, { does: "" }),
        "Part 1: say what this part of the code does.",
      ],
      [
        changePart(start, 0, { does: "d".repeat(801) }),
        "Part 1: what it does has at most 800 characters.",
      ],
      [
        changePart(start, 1, { files: "src/routes.rs" }),
        "src/routes.rs is in more than one part.",
      ],
      [
        changePart(start, 0, { files: "src/schemas.rs" }),
        "Put src/routes.rs in the part whose code it changes.",
      ],
      [
        changeSection(start, "events", "e\n".repeat(21)),
        "Events has at most 20 entries.",
      ],
      [
        changeSection(start, "not_changed", "x".repeat(301)),
        "Not changed: an entry has at most 300 characters.",
      ],
    ];
    for (const [draft, problem] of cases) {
      expect(draftProblem(draft, path)).toBe(problem);
    }
    expect(draftProblem({ ...start, summary: "é".repeat(1200) }, path)).toBe(
      null,
    );
  });
});
