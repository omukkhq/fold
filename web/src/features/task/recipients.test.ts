import { describe, expect, it } from "vitest";
import { recipientOptions } from "./recipients";

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

  it("is empty before the targets are read", () => {
    expect(recipientOptions(null, undefined)).toEqual([]);
  });
});
