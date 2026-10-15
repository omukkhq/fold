import { describe, expect, it } from "vitest";
import type { Revision } from "../../api/repository";
import { describeRevision, formatSize } from "./describe";

const NOW = new Date(2026, 9, 15, 15, 0, 0).getTime();
const AT = new Date(2026, 9, 15, 14, 6, 0).getTime();

function revision(integration: Revision["integration"]): Revision {
  return { id: "r8", number: 8, integration, integratedAt: AT };
}

describe("describeRevision", () => {
  it("names the proposal, the task and the agent for a proposal integration", () => {
    expect(
      describeRevision(
        revision({
          kind: "proposal",
          proposal: "P-5",
          task: "T-15",
          title: "Form field components",
          agent: "frontend/forms",
        }),
        NOW,
      ),
    ).toEqual({
      title: "P-5 · Form field components",
      meta: "T-15 · frontend/forms · 14:06:00",
    });
  });

  it("names pull, the remote commit and the human for a pull", () => {
    expect(
      describeRevision(
        revision({
          kind: "pull",
          remoteCommit: "9f2c1ab",
          human: "human:tanim",
        }),
        NOW,
      ),
    ).toEqual({
      title: "pull · 9f2c1ab",
      meta: "human:tanim · 14:06:00",
    });
  });

  it("shows the title of the first revision", () => {
    expect(
      describeRevision(
        revision({ kind: "initial", title: "Imported", human: "human:tanim" }),
        NOW,
      ).title,
    ).toBe("Imported");
  });
});

describe("formatSize", () => {
  it("scales bytes, kilobytes and megabytes", () => {
    expect(formatSize(12)).toBe("12 B");
    expect(formatSize(18_432)).toBe("18.0 KB");
    expect(formatSize(3 * 1024 * 1024)).toBe("3.0 MB");
  });
});
