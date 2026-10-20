import { describe, expect, it } from "vitest";
import { workerProcess } from "@/test/workers";
import { sessionCopy, sessionView, transcriptCopy } from "./session";

const ID = "0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13";
const PATH = `/home/tanim/.claude/projects/-home-tanim-ledger/${ID}.jsonl`;

describe("a worker's session", () => {
  it("shows the start of the id and copies the whole id", () => {
    const session = sessionView(
      workerProcess("web-lead", { session_id: ID, transcript: PATH }),
    );
    expect(session).toEqual({ id: ID, short: "0f8e2c1a", transcript: PATH });
    expect(sessionCopy(session)).toEqual({
      text: ID,
      label: "Copy session id 0f8e2c1a",
    });
    expect(transcriptCopy(session)).toEqual({
      text: PATH,
      label: "Copy transcript path of session 0f8e2c1a",
    });
  });

  it("has nothing to copy for a transcript the daemon cannot place", () => {
    const session = sessionView(
      workerProcess("web-lead", { session_id: ID, transcript: null }),
    );
    expect(session.transcript).toBeNull();
    expect(transcriptCopy(session)).toBeNull();
  });
});
