import { Check, Copy } from "lucide-react";
import { type ReactNode, useState } from "react";
import { copyOutcome, type CopyState } from "@/components/mono/commitCopy";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  type CopyTarget,
  type SessionView,
  sessionCopy,
  transcriptCopy,
} from "./session";

const KEY = "font-title text-13 leading-tight font-semibold text-text-2";

function CopyText({
  target,
  children,
}: {
  target: CopyTarget;
  children: ReactNode;
}) {
  const [state, setState] = useState<CopyState>("idle");
  return (
    <button
      type="button"
      aria-label={target.label}
      onClick={() => {
        void copyOutcome(target.text, navigator.clipboard).then(setState);
      }}
      onBlur={() => {
        setState("idle");
      }}
      className={cn(
        "-ml-6 inline-flex min-w-0 items-start gap-8 rounded-s px-6 py-2 text-left font-mono text-13 text-text hover:bg-surface",
        focusRing,
      )}
    >
      <span className="min-w-0 wrap-anywhere">{children}</span>
      {state === "copied" ? (
        <Check aria-hidden size={14} className="mt-2 shrink-0 text-ok" />
      ) : (
        <Copy aria-hidden size={14} className="mt-2 shrink-0 text-text-2" />
      )}
      <span role="status" className="sr-only">
        {state === "copied" ? "Copied" : state === "failed" ? "Not copied" : ""}
      </span>
    </button>
  );
}

/** The worker's Claude Code session id and the path of its transcript, each copied when clicked; they sit in the worker's open row as label and value pairs of a two-column grid. */
export function SessionLine({ session }: { session: SessionView }) {
  const transcript = transcriptCopy(session);
  return (
    <>
      <span className={KEY}>Session</span>
      <span className="min-w-0">
        <CopyText target={sessionCopy(session)}>{session.short}</CopyText>
      </span>
      <span className={KEY}>Transcript</span>
      {transcript === null ? (
        <span className="text-13 text-text-2">transcript path unknown</span>
      ) : (
        <span className="min-w-0">
          <CopyText target={transcript}>{transcript.text}</CopyText>
        </span>
      )}
    </>
  );
}
