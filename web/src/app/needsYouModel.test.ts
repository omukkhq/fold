import { describe, expect, it } from "vitest";
import { needsItem } from "@/test/workers";
import {
  GROUP_OF,
  fullTitle,
  itemsFor,
  needsYouGroups,
  reviewsWaitingFor,
  pathsText,
  questionText,
  titleText,
  waitingText,
  type NeedsYouGroup,
} from "./needsYouModel";

const items = [
  needsItem({
    kind: "permission",
    screen: "worker",
    question: "pq-3",
    agent: "web-worker",
    tool: "Bash",
    task: "T-12",
  }),
  needsItem({
    kind: "question",
    message: "m-34",
    thread: "m-30",
    agent: "web-lead",
    task: "T-21",
    choices: [
      { label: "Recharts", description: "Ready-made bars." },
      { label: "Plain SVG", description: null },
    ],
  }),
  needsItem({ kind: "question", message: "m-40", agent: "api-lead" }),
  needsItem({
    kind: "human_review",
    screen: "proposal",
    proposal: "P-4",
    human: "tanim",
    title: "Database schema for reports",
  }),
  needsItem({
    kind: "request_draft",
    screen: "request",
    request: "RQ-5",
    human: "ana",
    title: "Can the board show who changed a card?",
  }),
];

describe("itemsFor", () => {
  it("leaves out what waits for another named human", () => {
    expect(itemsFor(items, "tanim")).toHaveLength(4);
    expect(itemsFor(items, null)).toHaveLength(5);
  });
});

describe("itemsFor a question to named humans", () => {
  const asks = [
    needsItem({ kind: "question", message: "m-1", humans: ["ana", "bob"] }),
    needsItem({ kind: "question", message: "m-2" }),
  ];

  it("shows a question to ana and bob to them and not to carol, as their Messages counts do", () => {
    const shown = (human: string) =>
      itemsFor(asks, human).map((item) => item.message);
    expect(shown("ana")).toEqual(["m-1", "m-2"]);
    expect(shown("bob")).toEqual(["m-1", "m-2"]);
    expect(shown("carol")).toEqual(["m-2"]);
    expect(itemsFor(asks, null)).toHaveLength(2);
  });
});

describe("reviewsWaitingFor", () => {
  const reviews = [
    needsItem({ kind: "human_review", proposal: "P-1", human: "tanim" }),
    needsItem({ kind: "human_review", proposal: "P-2" }),
    needsItem({ kind: "human_review", proposal: "P-3", human: "ana" }),
    needsItem({ kind: "question", message: "m-4" }),
  ];

  it("counts the reviews that wait for this human or for any human", () => {
    expect(reviewsWaitingFor(reviews, "tanim")).toBe(2);
    expect(reviewsWaitingFor(reviews, "ana")).toBe(2);
    expect(reviewsWaitingFor(reviews, null)).toBe(3);
    expect(reviewsWaitingFor([], "tanim")).toBe(0);
  });
});

const allLines = (groups: NeedsYouGroup[]) => groups.flatMap((g) => g.lines);
const lineKinds = (groups: NeedsYouGroup[]) =>
  allLines(groups).map((line) => line.kind);

describe("needsYouGroups", () => {
  it("groups in group order and links each item to its screen", () => {
    const groups = needsYouGroups("demo", items);
    expect(groups.map((g) => g.title)).toEqual([
      "Agents ask you",
      "Waiting for you",
    ]);
    const [ask, wait] = groups;
    expect(ask?.lines.map((l) => [l.kind, l.text, l.href])).toEqual([
      ["question", "asks you to choose", "/p/demo/messages?thread=m-30"],
      ["question", "asks a question", "/p/demo/messages?thread=m-40"],
      ["permission", "asks to run Bash", "/p/demo/agents?question=pq-3"],
    ]);
    expect(ask?.lines[2]).toMatchObject({
      person: "web-worker",
      task: "T-12",
      inline: false,
    });
    expect(wait?.lines[0]).toMatchObject({
      chip: "P-4",
      person: null,
      text: "needs your review",
      title: "Database schema for reports",
      inline: true,
      href: "/p/demo/proposals/P-4",
    });
    expect(wait?.lines[1]).toMatchObject({
      chip: "RQ-5",
      text: "has a draft ready to approve",
      title: "Can the board show who changed a card?",
      inline: true,
      href: "/p/demo/requests/RQ-5",
    });
  });

  it("places every kind in one of the three groups", () => {
    expect(new Set(Object.values(GROUP_OF))).toEqual(
      new Set(["ask", "wait", "project"]),
    );
    expect(Object.keys(GROUP_OF)).toHaveLength(15);
  });

  it("drops an empty group and words a missing tool", () => {
    const groups = needsYouGroups("demo", [
      needsItem({ kind: "permission", agent: "w" }),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.lines[0]?.text).toBe("asks to use a tool");
    expect(groups[0]?.lines[0]?.href).toBe("/p/demo/agents?question=");
  });

  it("links a review or draft without its id to the list it belongs to", () => {
    const [group] = needsYouGroups("demo", [
      needsItem({ kind: "human_review" }),
      needsItem({ kind: "request_draft" }),
    ]);
    const [review, draft] = group?.lines ?? [];
    expect(review?.href).toBe("/p/demo/proposals/");
    expect(draft?.href).toBe("/p/demo/requests/");
    expect(needsYouGroups("demo", [needsItem()])[0]?.lines[0]?.href).toBe(
      "/p/demo/messages",
    );
  });

  it("names a question by its header", () => {
    const choices = [{ label: "Red", description: null }];
    expect(questionText({ header: "Chart library", choices })).toBe(
      "asks about Chart library",
    );
    expect(questionText({ header: null, choices })).toBe("asks you to choose");
    expect(questionText({ header: null, choices: [] })).toBe("asks a question");
  });

  it("trims a title for the second line and leaves a blank one out", () => {
    expect(titleText("  Padded  ")).toBe("Padded");
    expect(titleText("   ")).toBeNull();
    expect(titleText(null)).toBeNull();
  });

  it("gives a question or a permission no title", () => {
    const [group] = needsYouGroups("demo", [
      needsItem({ kind: "permission", agent: "w", title: "ignored" }),
    ]);
    expect(group?.lines[0]?.title).toBeNull();
  });

  it("lists ready human tasks and blockers last, each opening its task", () => {
    const groups = needsYouGroups("demo", [
      needsItem({
        kind: "blocker",
        screen: "task",
        task: "T-2",
        title: "Wire the Meta login",
        reason: "Wait for the app id",
      }),
      needsItem({ kind: "question", message: "m-1" }),
      needsItem({
        kind: "human_task",
        screen: "task",
        task: "T-1",
        title: "Verify the app on Meta",
      }),
    ]);
    expect(groups.map((g) => g.title)).toEqual([
      "Agents ask you",
      "Waiting for you",
    ]);
    expect(allLines(groups)[1]).toMatchObject({
      key: "human_task:T-1",
      chip: "T-1",
      person: null,
      task: null,
      text: "is ready for a person to do",
      title: "Verify the app on Meta",
      href: "/p/demo/t/T-1",
    });
    expect(allLines(groups)[2]).toMatchObject({
      key: "blocker:T-2",
      chip: "T-2",
      task: null,
      text: "has a blocker",
      title: "Wait for the app id",
      href: "/p/demo/t/T-2",
    });
    const [bare] = needsYouGroups("demo", [needsItem({ kind: "blocker" })]);
    expect(bare?.lines[0]).toMatchObject({
      title: null,
      href: "/p/demo/t/",
    });
    const [human] = needsYouGroups("demo", [needsItem({ kind: "human_task" })]);
    expect(human?.lines[0]?.href).toBe("/p/demo/t/");
  });

  it("links a conflicting human change to the history of main", () => {
    const [group] = needsYouGroups("demo", [
      needsItem({
        kind: "human_change",
        screen: "repository",
        human: "ana",
        commit: "9f2c4e1a7b3d5f6e8a0c2b4d6f8e0a1c3b5d7f9e",
        paths: ["README.md", "src/lib.rs"],
      }),
    ]);
    expect(group?.title).toBe("Project");
    expect(group?.lines[0]).toMatchObject({
      key: "human_change:9f2c4e1a7b3d5f6e8a0c2b4d6f8e0a1c3b5d7f9e",
      chip: "9f2c4e1",
      person: null,
      title: "README.md and 1 more",
      text: "conflicts with main; the main agent explains it",
      href: "/p/demo/repository",
    });
    const [bare] = needsYouGroups("demo", [
      needsItem({ kind: "human_change" }),
    ]);
    expect(bare?.lines[0]).toMatchObject({ chip: null, title: null });
    expect(pathsText([])).toBeNull();
    expect(pathsText(["src/forms/index.ts"])).toBe("src/forms/index.ts");
    expect(pathsText(["a.md", "b.md", "c.md"])).toBe("a.md and 2 more");
    expect(pathsText(["a`b.md", "c.md"])).toBe("a\\`b.md and 1 more");
    expect(titleText(" Wait for `x` ")).toBe("Wait for \\`x\\`");
    expect(questionText({ header: "Use `pnpm`?", choices: [] })).toBe(
      "asks about Use \\`pnpm\\`?",
    );
  });

  it("links a Git branch that fold did not move to the history of main with the command that brings it back", () => {
    const [group] = needsYouGroups("demo", [
      needsItem({
        kind: "head_moved",
        screen: "repository",
        reason: "run `git switch main` in the project folder",
      }),
    ]);
    expect(group?.title).toBe("Project");
    expect(group?.lines[0]).toMatchObject({
      person: null,
      chip: null,
      title: "run `git switch main` in the project folder",
      text: "fold does not update the project folder until the Git branch is back",
      href: "/p/demo/repository",
    });
  });

  it("links a project folder that waits on the human's changes to the history of main with the files that hold it back", () => {
    const [group] = needsYouGroups("demo", [
      needsItem({
        kind: "folder_held",
        screen: "repository",
        reason: "run `fld record`, or take the changes back",
        paths: ["README.md", "src/app.ts"],
      }),
    ]);
    expect(group?.title).toBe("Project");
    expect(group?.lines[0]).toMatchObject({
      person: null,
      chip: null,
      title: "README.md and 1 more",
      text: "Changes were made in your project. Undo the changes or save them using `fld record`",
      href: "/p/demo/repository",
    });
  });

  it("shows an unstash whose conflict markers wait with the conflicted files, opening nothing", () => {
    const [group] = needsYouGroups("demo", [
      needsItem({
        kind: "unstash_conflicted",
        screen: "repository",
        reason:
          "fix the conflict markers in these files, then run `fld record`",
        paths: ["src/forms/index.ts", "src/forms/a.ts", "src/forms/b.ts"],
      }),
    ]);
    expect(group?.title).toBe("Project");
    expect(group?.lines[0]).toMatchObject({
      person: null,
      chip: null,
      task: null,
      title: "src/forms/index.ts and 2 more",
      text: "Unstash conflicts. Fix the files and run `fld record`",
      href: null,
    });
  });

  it("shows main's invalid checks file with its file, line and problem, opening nothing", () => {
    const [group] = needsYouGroups("demo", [
      needsItem({
        kind: "checks_file_invalid_on_main",
        screen: "repository",
        title: "fold-checks.toml line 12: unknown field `tiemout`",
        paths: ["fold-checks.toml"],
      }),
    ]);
    expect(group?.title).toBe("Project");
    expect(group?.lines[0]).toMatchObject({
      person: null,
      chip: null,
      task: null,
      title: "fold-checks.toml line 12: unknown field \\`tiemout\\`",
      text: "Integrations wait until the checks file on main is fixed",
      href: null,
    });
  });

  it("puts an unstash conflict, then main's checks file, after the blockers", () => {
    const groups = needsYouGroups("demo", [
      needsItem({ kind: "checks_file_invalid_on_main", paths: ["a.toml"] }),
      needsItem({ kind: "unstash_conflicted", paths: ["a.md"] }),
      needsItem({ kind: "blocker", task: "T-3" }),
    ]);
    expect(lineKinds(groups)).toEqual([
      "blocker",
      "unstash_conflicted",
      "checks_file_invalid_on_main",
    ]);
  });

  it("shows a worker that cannot reach fold with the command that brings it back, last of the groups", () => {
    const groups = needsYouGroups("demo", [
      needsItem({
        kind: "worker_unreachable",
        screen: "worker",
        agent: "backend-lead",
        reason: "run `fld restart`; the daemon then resumes the worker",
      }),
      needsItem({ kind: "checks_file_invalid_on_main", paths: ["a.toml"] }),
    ]);
    expect(lineKinds(groups)).toEqual([
      "checks_file_invalid_on_main",
      "worker_unreachable",
    ]);
    expect(groups.map((g) => g.title)).toEqual(["Project"]);
    expect(allLines(groups)[1]).toMatchObject({
      person: "backend-lead",
      chip: null,
      task: null,
      title: "run `fld restart`; the daemon then resumes the worker",
      text: "cannot reach fold and stopped",
      href: "/p/demo/agents",
    });
  });

  it("shows a worker that cannot reach the model with Claude Code's error and what to check, after the workers that cannot reach fold", () => {
    const groups = needsYouGroups("demo", [
      needsItem({
        kind: "worker_model_unreachable",
        screen: "worker",
        agent: "web-lead",
        title: "Invalid `key`",
        reason: "check the sign-in, or run `fld doctor`",
      }),
      needsItem({ kind: "worker_unreachable", agent: "api-lead" }),
      needsItem({ kind: "worker_model_unreachable", agent: "api-worker" }),
    ]);
    expect(lineKinds(groups)).toEqual([
      "worker_unreachable",
      "worker_model_unreachable",
      "worker_model_unreachable",
    ]);
    expect(groups.map((g) => g.title)).toEqual(["Project"]);
    expect(allLines(groups)[1]).toMatchObject({
      person: "web-lead",
      chip: null,
      task: null,
      title: "Invalid \\`key\\`",
      text: "cannot reach the model: check the sign-in, or run `fld doctor`",
      href: "/p/demo/agents",
    });
    expect(allLines(groups)[2]).toMatchObject({
      person: "api-worker",
      title: null,
      text: "cannot reach the model",
    });
  });

  it("shows a worker whose claude the daemon cannot find with the fix, after the worker connections", () => {
    const groups = needsYouGroups("demo", [
      needsItem({
        kind: "claude_not_found",
        screen: "worker",
        agent: "web-worker",
        reason: "install Claude Code, then run `fld restart`",
      }),
      needsItem({
        kind: "worker_unreachable",
        screen: "worker",
        agent: "backend-lead",
        reason: "run `fld restart`",
      }),
    ]);
    expect(lineKinds(groups)).toEqual([
      "worker_unreachable",
      "claude_not_found",
    ]);
    expect(groups.map((g) => g.title)).toEqual(["Project"]);
    expect(allLines(groups)[1]).toMatchObject({
      person: "web-worker",
      chip: null,
      task: null,
      title: "install Claude Code, then run `fld restart`",
      text: "cannot start: the daemon finds no claude",
      href: "/p/demo/agents",
    });
  });

  it("shows a notice target fold turned off in the project group, last, with a link to the settings", () => {
    const groups = needsYouGroups("demo", [
      needsItem({
        kind: "notify_target_off",
        screen: "settings",
        target: "nt-3",
        title: "Release bot",
      }),
      needsItem({
        kind: "claude_not_found",
        screen: "worker",
        agent: "web-worker",
      }),
      needsItem({ kind: "blocker", screen: "task", task: "T-1" }),
    ]);
    expect(groups.map((g) => g.id)).toEqual(["wait", "project"]);
    expect(groups[1]?.lines.map((line) => line.kind)).toEqual([
      "claude_not_found",
      "notify_target_off",
    ]);
    expect(groups[1]?.lines[1]).toMatchObject({
      person: null,
      chip: "nt-3",
      task: null,
      title: "Release bot",
      href: "/p/demo/settings",
    });
    expect(groups[1]?.lines[1]).not.toHaveProperty("hint");
  });

  it("counts what waits", () => {
    expect(waitingText(4)).toBe("4 waiting");
  });
});

describe("fullTitle", () => {
  it("gives the whole title as plain words, its commands without their marks", () => {
    expect(fullTitle({ title: "Record changes in an audit log" })).toBe(
      "Record changes in an audit log",
    );
    expect(
      fullTitle({ title: "fold-checks.toml line 12: unknown field `tiemout`" }),
    ).toBe("fold-checks.toml line 12: unknown field tiemout");
    expect(fullTitle({ title: "a \\`b\\` path" })).toBe("a `b` path");
    expect(fullTitle({ title: null })).toBeNull();
  });
});
