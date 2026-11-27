import { describe, expect, it } from "vitest";
import {
  type IntegrationQueuePlace,
  type IntegrationWait,
  ordinal,
  queueWait,
} from "./queueWait";

const NOW = 1_760_000_000_000;
const MIN = 60_000;

function place(fields: Partial<IntegrationQueuePlace>): IntegrationQueuePlace {
  return {
    position: 1,
    ahead: null,
    batch: null,
    estimated_start: null,
    integrating_now: false,
    ...fields,
  };
}

function text(wait: IntegrationWait | null): string | null {
  return queueWait({ queue: null, waits_for: wait }, NOW)?.text ?? null;
}

describe("ordinal", () => {
  it.each([
    [1, "1st"],
    [2, "2nd"],
    [3, "3rd"],
    [4, "4th"],
    [11, "11th"],
    [12, "12th"],
    [13, "13th"],
    [21, "21st"],
    [22, "22nd"],
    [101, "101st"],
    [111, "111th"],
  ])("%i is %s", (n, expected) => {
    expect(ordinal(n)).toBe(expected);
  });
});

describe("queueWait place in the queue", () => {
  it("names position 1 as the next turn", () => {
    expect(queueWait({ queue: place({}), waits_for: null }, NOW)).toEqual({
      state: "queue",
      title: "Next in line",
      detail: "",
      text: "Next in line",
    });
  });

  it("gives position 1 its estimated start", () => {
    const wait = queueWait(
      { queue: place({ estimated_start: NOW + 4 * MIN + 20_000 }) },
      NOW,
    );
    expect(wait?.text).toBe("Next in line · starts in about 4 min");
  });

  it("says that the turn starts any moment once the estimate has passed", () => {
    const wait = queueWait(
      { queue: place({ estimated_start: NOW - MIN }) },
      NOW,
    );
    expect(wait?.detail).toBe("starts any moment");
  });

  it("names the turn that runs now, with its batch", () => {
    expect(
      queueWait({ queue: place({ integrating_now: true }) }, NOW)?.text,
    ).toBe("Integrating now · the integration checks run");
    expect(
      queueWait(
        {
          queue: place({
            integrating_now: true,
            batch: "rb-2",
            estimated_start: NOW - MIN,
          }),
        },
        NOW,
      )?.text,
    ).toBe("Integrating now · the integration checks run · with batch rb-2");
  });

  it("names a later position, the proposal ahead and the estimate", () => {
    const wait = queueWait(
      {
        queue: place({
          position: 2,
          ahead: "P-12",
          estimated_start: NOW + 10 * MIN,
        }),
      },
      NOW,
    );
    expect(wait).toEqual({
      state: "queue",
      title: "2nd in line",
      detail: "after P-12 · starts in about 10 min",
      text: "2nd in line · after P-12 · starts in about 10 min",
    });
  });

  it("names a later position without an estimate", () => {
    expect(
      queueWait({ queue: place({ position: 3, ahead: "rb-1" }) }, NOW)?.text,
    ).toBe("3rd in line · after rb-1");
  });

  it("leaves out the proposal ahead when the place names none, and names the batch", () => {
    expect(
      queueWait({ queue: place({ position: 4, batch: "rb-2" }) }, NOW)?.text,
    ).toBe("4th in line · with batch rb-2");
    expect(queueWait({ queue: place({ batch: "rb-2" }) }, NOW)?.text).toBe(
      "Next in line · with batch rb-2",
    );
  });

  it("prefers the place over a wait reason", () => {
    expect(
      queueWait(
        { queue: place({}), waits_for: { reason: "not_approved" } },
        NOW,
      )?.state,
    ).toBe("queue");
  });
});

describe("queueWait reasons", () => {
  it("gives nothing for a proposal with neither field", () => {
    expect(queueWait({ queue: null, waits_for: null }, NOW)).toBeNull();
    expect(queueWait({}, NOW)).toBeNull();
  });

  it.each<[IntegrationWait, string]>([
    [
      { reason: "automatic_off" },
      "Automatic integration is off · a human integrates it",
    ],
    [
      { reason: "config_invalid" },
      "Project config is invalid · automatic integration waits until it is valid",
    ],
    [
      { reason: "main_checks_invalid" },
      "The checks file of main is invalid · automatic integration waits until it is valid",
    ],
    [{ reason: "conflict" }, "Conflict with main · it waits for a new version"],
    [
      { reason: "checks_failed" },
      "Integration checks failed · it waits for a new version",
    ],
    [
      { reason: "lane", lane: "migrations", position: 2 },
      "Waits in lane migrations · position 2",
    ],
    [
      { reason: "batch", batch: "rb-2" },
      "Waits for batch rb-2 · the batch integrates together",
    ],
    [
      { reason: "fold_failed_checks" },
      "Checks did not finish · integration waits until they run again",
    ],
    [
      { reason: "next_version_run" },
      "Checks of the next version run · integration waits for them",
    ],
    [
      {
        reason: "prerequisites",
        prerequisites: [
          { task: "T-4", status: "in_progress" },
          { task: "T-9", status: "todo" },
        ],
      },
      "Waits for prerequisites · not done: T-4, T-9",
    ],
    [{ reason: "prerequisites", prerequisites: [] }, "Waits for prerequisites"],
    [{ reason: "not_approved" }, "Not approved · it waits for a review"],
  ])("%j reads as one waiting line", (wait, expected) => {
    expect(text(wait)).toBe(expected);
    expect(queueWait({ waits_for: wait }, NOW)?.state).toBe("waiting");
  });

  it("marks a wait for a human as a wait on a person", () => {
    expect(
      queueWait(
        { waits_for: { reason: "human_review", reviewer: "tanim" } },
        NOW,
      ),
    ).toEqual({
      state: "person",
      title: "Waits for human review",
      detail: "by tanim",
      text: "Waits for human review · by tanim",
    });
    expect(
      queueWait({ waits_for: { reason: "human_review", reviewer: null } }, NOW)
        ?.detail,
    ).toBe("any human");
  });
});
