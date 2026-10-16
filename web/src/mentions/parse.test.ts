import { describe, expect, it } from "vitest";
import {
  codeRanges,
  findMentions,
  isCleanPath,
  isPath,
  isPerson,
  knownFrom,
  mentionHref,
  splitMentions,
  type Mention,
} from "./parse";

const texts = (found: Mention[]) => found.map((m) => m.text);

describe("findMentions", () => {
  it("finds every item kind after # with exact spans", () => {
    const text = "#T-12 #P-3 #RQ-2 #r17 #ws-9 #ck-88 #m-210 #C-40";
    const found = findMentions(text);
    expect(found.map((m) => (m.mark === "#" ? [m.kind, m.id] : null))).toEqual([
      ["task", "T-12"],
      ["proposal", "P-3"],
      ["request", "RQ-2"],
      ["revision", "r17"],
      ["workspace", "ws-9"],
      ["checkpoint", "ck-88"],
      ["message", "m-210"],
      ["changeset", "C-40"],
    ]);
    for (const m of found) expect(text.slice(m.start, m.end)).toBe(m.text);
  });

  it("leaves a # that names nothing, or sits inside a word, as text", () => {
    expect(
      findMentions("#rc-5 #T-0 #T-012 #x page#T-1 &#12; #T-12a #T-1-b ##T-3"),
    ).toEqual([]);
  });

  it("finds main-tree and workspace paths after @", () => {
    const found = findMentions(
      "See @web/src/board.ts, @Makefile.am and @ws-9:web/src/a.ts.",
    );
    expect(texts(found)).toEqual([
      "@web/src/board.ts",
      "@Makefile.am",
      "@ws-9:web/src/a.ts",
    ]);
    const ws = found[2];
    expect(ws !== undefined && isPath(ws) && [ws.workspace, ws.path]).toEqual([
      "ws-9",
      "web/src/a.ts",
    ]);
  });

  it("leaves names, e-mail addresses and unclean paths as text", () => {
    expect(
      findMentions("@tanim mail a@b.com @/etc/passwd @../x @a//b @./ @"),
    ).toEqual([]);
  });

  it("orders # and @ mentions by position", () => {
    expect(texts(findMentions("@a/b.ts then #T-1"))).toEqual([
      "@a/b.ts",
      "#T-1",
    ]);
  });
});

describe("isCleanPath", () => {
  it.each(["a", "a/b.ts", "web/src/x-y_z.tsx"])("accepts %j", (path) => {
    expect(isCleanPath(path)).toBe(true);
  });
  it.each(["", "/a", "a/", "a//b", "./a", "a/../b", ".."])(
    "rejects %j",
    (path) => {
      expect(isCleanPath(path)).toBe(false);
    },
  );
});

describe("codeRanges", () => {
  it("pairs runs of the same length and leaves a lone run as text", () => {
    const text = "a `x` b ``y ` z`` c ` d";
    const ranges = codeRanges(text);
    expect(ranges.map(([from, to]) => text.slice(from, to))).toEqual([
      "`x`",
      "``y ` z``",
    ]);
  });
});

describe("splitMentions", () => {
  it("splits text and mentions and keeps every character", () => {
    const text = "Use #T-9 and @web/a.ts now";
    const segments = splitMentions(text);
    expect(segments.map((s) => [s.text, s.mention !== undefined])).toEqual([
      ["Use ", false],
      ["#T-9", true],
      [" and ", false],
      ["@web/a.ts", true],
      [" now", false],
    ]);
    expect(segments.map((s) => s.text).join("")).toBe(text);
  });

  it("keeps a mention inside a code span as text", () => {
    const segments = splitMentions("`#T-9` and #T-10");
    expect(segments.filter((s) => s.mention).map((s) => s.text)).toEqual([
      "#T-10",
    ]);
  });

  it("keeps a mention the known check rejects as text", () => {
    const known = knownFrom({ requests: ["RQ-1"] });
    const segments = splitMentions("#RQ-1 #RQ-9", known);
    expect(segments.filter((s) => s.mention).map((s) => s.text)).toEqual([
      "#RQ-1",
    ]);
    expect(splitMentions("")).toEqual([]);
  });
});

describe("mentionHref", () => {
  const href = (text: string) => {
    const [m] = findMentions(text);
    return m ? mentionHref("demo", m) : undefined;
  };

  it.each([
    ["#T-12", "/p/demo/t/T-12"],
    ["#P-3", "/p/demo/proposals/P-3"],
    ["#RQ-2", "/p/demo/requests/RQ-2"],
    ["#r17", "/p/demo/repository?revision=r17"],
    ["#m-210", "/p/demo/messages?thread=m-210"],
    ["#C-40", "/p/demo/diff?to=C-40"],
    ["#ws-9", "/p/demo/diff?to=ws-9"],
    ["#ck-88", "/p/demo/diff?to=ck-88"],
    ["@web/src/a.ts", "/p/demo/repository?path=web%2Fsrc%2Fa.ts"],
    ["@ws-9:web/a.ts", "/p/demo/diff?to=ws-9&path=web%2Fa.ts"],
  ])("links %s to %s", (text, target) => {
    expect(href(text)).toBe(target);
  });
});

describe("knownFrom", () => {
  it("rejects only ids missing from a list the screen holds", () => {
    const known = knownFrom({ proposals: ["P-1"] });
    const [p1, p2, rq, task, path] = findMentions(
      "#P-1 #P-2 #RQ-5 #T-3 @a/b.ts",
    );
    expect([p1, p2, rq, task, path].map((m) => m && known(m))).toEqual([
      true,
      false,
      true,
      true,
      true,
    ]);
  });
});

describe("person mentions", () => {
  const people = {
    humans: ["tanim", "Makefile", "notes.md"],
    agents: ["frontend", "frontend/ui"],
  };
  const shape = (text: string) =>
    findMentions(text, people).map((m) =>
      isPerson(m)
        ? ["person", m.person, m.name]
        : isPath(m)
          ? ["path", m.path, m.text]
          : ["item", m.text],
    );

  it("names a known human or agent before a file", () => {
    expect(shape("ask @tanim and @frontend, @frontend/ui too")).toEqual([
      ["person", "human", "tanim"],
      ["person", "agent", "frontend"],
      ["person", "agent", "frontend/ui"],
    ]);
  });

  it("leaves an unknown name as text and an unknown dotted name as a file", () => {
    expect(shape("@nobody @a.ts")).toEqual([["path", "a.ts", "@a.ts"]]);
  });

  it("reads a trailing slash as a folder, even for a person's name", () => {
    expect(shape("@frontend/ @web/src/ @tanim/")).toEqual([
      ["path", "frontend", "@frontend/"],
      ["path", "web/src", "@web/src/"],
      ["path", "tanim", "@tanim/"],
    ]);
  });

  it("reads @./name as a file, also for a person named like a root file", () => {
    expect(shape("@Makefile @./Makefile @notes.md @./notes.md")).toEqual([
      ["person", "human", "Makefile"],
      ["path", "Makefile", "@./Makefile"],
      ["person", "human", "notes.md"],
      ["path", "notes.md", "@./notes.md"],
    ]);
  });

  it("drops sentence punctuation after a name", () => {
    expect(shape("thanks @tanim.")).toEqual([["person", "human", "tanim"]]);
  });

  it("splits text around a person and links an agent to its board", () => {
    const [, person] = splitMentions("hi @frontend now", undefined, people);
    expect(person?.mention && mentionHref("demo", person.mention)).toBe(
      "/p/demo/board?agent=frontend",
    );
    const [, worker] = splitMentions("hi @frontend/ui now", undefined, people);
    expect(worker?.mention && mentionHref("demo", worker.mention)).toBe(
      "/p/demo/board?agent=frontend",
    );
    const [, human] = splitMentions("hi @tanim now", undefined, people);
    expect(human?.mention && mentionHref("demo", human.mention)).toBeNull();
  });

  it("finds no person without a list of names", () => {
    expect(findMentions("@tanim")).toEqual([]);
  });
});
