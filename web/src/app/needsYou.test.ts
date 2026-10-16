import { describe, expect, it } from "vitest";
import { needsItem } from "@/test/workers";
import {
  itemsFor,
  needsYouGroups,
  questionText,
  titleText,
  waitingText,
} from "./needsYou";

const items = [
  needsItem({
    kind: "permission",
    screen: "worker",
    question: "pq-3",
    agent: "web-coder",
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

describe("needsYouGroups", () => {
  it("groups in group order and links each item to its screen", () => {
    const groups = needsYouGroups("demo", items);
    expect(groups.map((g) => g.title)).toEqual([
      "Questions",
      "Permissions",
      "Reviews",
      "Request drafts",
    ]);
    const [questions, permissions, reviews, drafts] = groups;
    expect(questions?.lines.map((l) => [l.text, l.href])).toEqual([
      ["asks you to choose", "/p/demo/messages?thread=m-30"],
      ["asks a question", "/p/demo/messages?thread=m-40"],
    ]);
    expect(permissions?.lines[0]).toMatchObject({
      person: "web-coder",
      text: "asks to run Bash",
      task: "T-12",
      hint: "opens the permission panel",
      href: "/p/demo/agents?question=pq-3",
    });
    expect(reviews?.lines[0]).toMatchObject({
      chip: "P-4",
      person: null,
      text: "needs your review",
      title: "Database schema for reports",
      href: "/p/demo/proposals/P-4",
    });
    expect(drafts?.lines[0]).toMatchObject({
      chip: "RQ-5",
      text: "has a draft ready to approve",
      title: "Can the board show who changed a card?",
      href: "/p/demo/requests/RQ-5",
    });
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
    const [review, draft] = needsYouGroups("demo", [
      needsItem({ kind: "human_review" }),
      needsItem({ kind: "request_draft" }),
    ]);
    expect(review?.lines[0]?.href).toBe("/p/demo/proposals/");
    expect(draft?.lines[0]?.href).toBe("/p/demo/requests/");
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
      "Questions",
      "Human tasks",
      "Blockers",
    ]);
    expect(groups[1]?.lines[0]).toMatchObject({
      key: "human_task:T-1",
      chip: "T-1",
      person: null,
      task: null,
      text: "is ready for a person to do",
      title: "Verify the app on Meta",
      hint: "opens the task",
      href: "/p/demo/t/T-1",
    });
    expect(groups[2]?.lines[0]).toMatchObject({
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

  it("counts what waits", () => {
    expect(waitingText(4)).toBe("4 waiting");
  });
});
