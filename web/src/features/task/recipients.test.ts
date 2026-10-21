import { describe, expect, it } from "vitest";
import { feedbackReaders, recipientOptions } from "./recipients";

const labels = (options: { label: string }[]) => options.map((o) => o.label);

describe("recipientOptions", () => {
  it("lists the main agent once when it holds the claim", () => {
    const options = recipientOptions("main", {
      agents: ["main", "backend"],
      tags: ["main", "review"],
    });
    expect(labels(options)).toEqual([
      "main (claim holder)",
      "backend",
      "tag review",
    ]);
    expect(options.map((o) => o.value)).toEqual([
      "holder",
      "agent:backend",
      "tag:review",
    ]);
  });

  it("lists another holder first and leaves it out of the agents", () => {
    expect(
      labels(
        recipientOptions("backend", {
          agents: ["main", "backend"],
          tags: ["main"],
        }),
      ),
    ).toEqual(["backend (claim holder)", "main"]);
  });

  it("offers no holder entry without a claim, and no main tag", () => {
    expect(
      labels(recipientOptions(null, { agents: ["main"], tags: ["main"] })),
    ).toEqual(["main"]);
  });

  it("keeps a tag that reaches several agents", () => {
    expect(
      labels(
        recipientOptions(null, { agents: ["a", "b"], tags: ["ui", "review"] }),
      ),
    ).toEqual(["a", "b", "tag ui", "tag review"]);
  });

  it("marks a stopped task worker, which starts again for the feedback", () => {
    const stopped = new Set(["backend-worker-10"]);
    expect(
      labels(
        recipientOptions(
          "backend-worker-10",
          { agents: ["backend-worker-10", "backend-lead", "main"], tags: [] },
          stopped,
        ),
      ),
    ).toEqual([
      "backend-worker-10 (claim holder) · stopped",
      "backend-lead",
      "main",
    ]);
    expect(
      labels(
        recipientOptions(
          null,
          { agents: ["backend-worker-10/ui"], tags: [] },
          stopped,
        ),
      ),
    ).toEqual(["backend-worker-10/ui · stopped"]);
  });

  it("is empty before the targets are read", () => {
    expect(recipientOptions(null, undefined)).toEqual([]);
  });

  it("names the agent a feedback choice reaches", () => {
    expect(feedbackReaders({ kind: "holder" }, "web-worker")).toEqual([
      "web-worker",
    ]);
    expect(feedbackReaders({ kind: "holder" }, null)).toEqual([]);
    expect(feedbackReaders({ kind: "agent", name: "web-lead" }, null)).toEqual([
      "web-lead",
    ]);
    expect(feedbackReaders({ kind: "tag", name: "ui" }, null)).toEqual([]);
    expect(feedbackReaders(null, "web-worker")).toEqual([]);
  });
});
