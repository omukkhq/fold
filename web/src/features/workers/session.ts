import type { WorkerProcess } from "@/api/workers";

/** A worker's Claude Code session as its row shows it. */
export interface SessionView {
  id: string;
  /** The start of the id, enough to tell the sessions of one project apart. */
  short: string;
  /** The transcript file; null when the daemon cannot tell where Claude Code keeps it. */
  transcript: string | null;
}

const SHORT = 8;

export function sessionView(
  worker: Pick<WorkerProcess, "session_id" | "transcript">,
): SessionView {
  return {
    id: worker.session_id,
    short: worker.session_id.slice(0, SHORT),
    transcript: worker.transcript,
  };
}

/** What a copy control of the row copies and how a screen reader names it. */
export interface CopyTarget {
  text: string;
  label: string;
}

export function sessionCopy(session: SessionView): CopyTarget {
  return { text: session.id, label: `Copy session id ${session.short}` };
}

/** The transcript path to copy; null when the daemon names no path. */
export function transcriptCopy(session: SessionView): CopyTarget | null {
  return session.transcript === null
    ? null
    : {
        text: session.transcript,
        label: `Copy transcript path of session ${session.short}`,
      };
}
