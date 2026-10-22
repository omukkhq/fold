import { describe, expect, it } from "vitest";
import { MAIN_AGENT_NOTE, mainAgentNote } from "./note";

describe("mainAgentNote", () => {
  it("says the user's own main agent is not counted", () => {
    expect(mainAgentNote(false)).toBe(MAIN_AGENT_NOTE);
  });

  it("says nothing when fold runs the main agent and counts it", () => {
    expect(mainAgentNote(true)).toBeNull();
  });

  it("says nothing while the workers list is not read yet", () => {
    expect(mainAgentNote(undefined)).toBeNull();
  });
});
