import { describe, expect, it } from "vitest";
import type { TaskReader } from "@/api/tasks";
import {
  keptChoice,
  readerLabel,
  readerRecipient,
  recipientOptions,
} from "./recipients";

const holder: TaskReader = { name: "web-t12", role: "claim_holder" };
const lead: TaskReader = { name: "web-lead", role: "area_lead" };
const main: TaskReader = { name: "main", role: "main_agent" };

describe("recipientOptions", () => {
  it("keeps the daemon's order, the claim holder first", () => {
    const options = recipientOptions([holder, lead, main]);
    expect(options.map((o) => o.label)).toEqual([
      "web-t12 (claim holder)",
      "web-lead (area lead)",
      "main (main agent)",
    ]);
    expect(options.map((o) => o.value)).toEqual([
      "holder",
      "agent:web-lead",
      "agent:main",
    ]);
  });

  it("is empty before the readers are read", () => {
    expect(recipientOptions(undefined)).toEqual([]);
    expect(recipientOptions([])).toEqual([]);
  });
});

describe("readerRecipient", () => {
  it("addresses the claim holder as the holder and the others by name", () => {
    expect(readerRecipient(holder)).toEqual({ kind: "holder" });
    expect(readerRecipient(lead)).toEqual({ kind: "agent", name: "web-lead" });
  });
});

describe("readerLabel", () => {
  it("names the reader and its role", () => {
    expect(readerLabel(lead)).toBe("web-lead (area lead)");
  });
});

describe("keptChoice", () => {
  const options = recipientOptions([lead, main]);

  it("keeps a choice the daemon still offers", () => {
    expect(keptChoice("agent:main", options)).toBe("agent:main");
  });

  it("falls back to the default when the choice is gone or not made", () => {
    expect(keptChoice("holder", options)).toBe("agent:web-lead");
    expect(keptChoice("", options)).toBe("agent:web-lead");
  });

  it("chooses no reader when the daemon offers none", () => {
    expect(keptChoice("agent:main", [])).toBe("");
  });
});
