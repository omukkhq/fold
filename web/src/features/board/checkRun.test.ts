import { describe, expect, it } from "vitest";
import {
  INTEGRATION_CHECKS_LABEL,
  PROPOSAL_CHECKS_LABEL,
  checkRunLabel,
  checkRunText,
  checkStage,
  currentCheckRun,
  type CheckRunView,
} from "./checkRun";

function run(over: Partial<CheckRunView> = {}): CheckRunView {
  return {
    run: "cr-4",
    purpose: "check",
    proposal: null,
    started_at: 1_000,
    ...over,
  };
}

describe("currentCheckRun", () => {
  it("takes the live frame's answer over the payload's, also when no run goes", () => {
    const payload = run();
    const streamed = run({ run: "cr-5" });
    expect(currentCheckRun(payload, { check_run: streamed })).toBe(streamed);
    expect(currentCheckRun(payload, { check_run: null })).toBeNull();
  });

  it("falls back to the payload without a live frame", () => {
    const payload = run();
    expect(currentCheckRun(payload, undefined)).toBe(payload);
    expect(currentCheckRun(payload, null)).toBe(payload);
    expect(currentCheckRun(null, undefined)).toBeNull();
  });
});

describe("checkRunText", () => {
  const proposal = { id: "P-3", version: 2 };

  it("names the version a proposal command waits to make", () => {
    expect(checkRunText(run({ purpose: "create_proposal" }), null)).toBe(
      "Running checks for v1",
    );
    expect(
      checkRunText(
        run({ purpose: "update_proposal", proposal: "P-3" }),
        proposal,
      ),
    ).toBe("Running checks for v3");
  });

  it("names no version for a worker's own run or a proposal it does not know", () => {
    expect(checkRunText(run(), proposal)).toBe("Running checks");
    expect(
      checkRunText(run({ purpose: "update_proposal", proposal: "P-3" }), null),
    ).toBe("Running checks");
    expect(
      checkRunText(
        run({ purpose: "update_proposal", proposal: "P-9" }),
        proposal,
      ),
    ).toBe("Running checks");
  });
});

describe("checkStage and checkRunLabel", () => {
  it("labels a proposal-stage run `Checks running`", () => {
    const proposalRun = run({ stage: "proposal", purpose: "update_proposal" });
    expect(checkStage(proposalRun)).toBe("proposal");
    expect(checkRunLabel(proposalRun)).toBe(PROPOSAL_CHECKS_LABEL);
    expect(PROPOSAL_CHECKS_LABEL).toBe("Checks running");
  });

  it("labels an integration-stage run `Integration checks running`", () => {
    const integration = run({ stage: "integration", purpose: "integrate" });
    expect(checkStage(integration)).toBe("integration");
    expect(checkRunLabel(integration)).toBe(INTEGRATION_CHECKS_LABEL);
    expect(INTEGRATION_CHECKS_LABEL).toBe("Integration checks running");
  });

  it("reads the stage from the purpose of a run that names none", () => {
    expect(checkStage(run({ purpose: "integrate" }))).toBe("integration");
    expect(checkStage(run({ purpose: "check" }))).toBe("proposal");
    expect(checkRunLabel(run({ purpose: "integrate" }))).toBe(
      INTEGRATION_CHECKS_LABEL,
    );
  });
});

describe("an integration run", () => {
  const integration = run({
    stage: "integration",
    purpose: "integrate",
    proposal: "P-3",
  });

  it("stays when the live frame of the workspace says no run goes", () => {
    expect(currentCheckRun(integration, { check_run: null })).toBe(integration);
    expect(currentCheckRun(integration, { check_run: run() })).toBe(
      integration,
    );
    expect(currentCheckRun(integration, undefined)).toBe(integration);
  });

  it("names no next version in its text", () => {
    expect(checkRunText(integration, { id: "P-3", version: 2 })).toBe(
      "Integration checks running",
    );
  });
});

describe("no run", () => {
  it("shows no state without a payload run and without a live run", () => {
    expect(currentCheckRun(null, { check_run: null })).toBeNull();
    expect(currentCheckRun(null, undefined)).toBeNull();
  });

  it("a proposal-stage payload run ends with the live frame's answer", () => {
    const proposalRun = run({ stage: "proposal" });
    expect(currentCheckRun(proposalRun, undefined)).toBe(proposalRun);
    expect(currentCheckRun(proposalRun, { check_run: null })).toBeNull();
  });
});
