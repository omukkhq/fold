// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SessionLine } from "./SessionLine";

afterEach(cleanup);

const ID = "0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13";
const PATH = `/home/tanim/.claude/projects/-home-tanim-ledger/${ID}.jsonl`;

describe("SessionLine", () => {
  it("shows the session and the transcript path and copies each", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    render(
      <SessionLine session={{ id: ID, short: "0f8e2c1a", transcript: PATH }} />,
    );
    const session = screen.getByRole("button", {
      name: "Copy session id 0f8e2c1a",
    });
    expect(session.textContent).toContain("session 0f8e2c1a");
    const transcript = screen.getByRole("button", {
      name: "Copy transcript path of session 0f8e2c1a",
    });
    expect(transcript.textContent).toContain(PATH);
    fireEvent.click(transcript);
    expect(await screen.findByText("Copied")).toBeTruthy();
    expect(writeText).toHaveBeenCalledWith(PATH);
    fireEvent.click(session);
    expect(writeText).toHaveBeenLastCalledWith(ID);
  });

  it("says when the daemon cannot place the transcript", () => {
    render(
      <SessionLine session={{ id: ID, short: "0f8e2c1a", transcript: null }} />,
    );
    expect(screen.getByText("transcript path unknown")).toBeTruthy();
    expect(screen.getAllByRole("button")).toHaveLength(1);
  });
});
