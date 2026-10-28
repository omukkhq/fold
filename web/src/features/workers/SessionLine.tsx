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

function CopyText({
  target,
  className,
  children,
}: {
  target: CopyTarget;
  className?: string;
  children: ReactNode;
}) {
  const [state, setState] = useState<CopyState>("idle");
  return (
    <button
      type="button"
      title={target.text}
      aria-label={target.label}
      onClick={() => {
        void copyOutcome(target.text, navigator.clipboard).then(setState);
      }}
      onBlur={() => {
        setState("idle");
      }}
      className={cn(
        "inline-flex min-w-0 items-center gap-4 rounded-s font-mono text-13 whitespace-nowrap text-text-2 hover:text-text",
        focusRing,
        className,
      )}
    >
      {children}
      {state === "copied" ? (
        <Check aria-hidden size={12} className="shrink-0" />
      ) : (
        <Copy aria-hidden size={12} className="shrink-0 text-text-2" />
      )}
      <span role="status" className="sr-only">
        {state === "copied" ? "Copied" : state === "failed" ? "Not copied" : ""}
      </span>
    </button>
  );
}

/** The worker's Claude Code session id and the path of its transcript, each copied when clicked. */
export function SessionLine({ session }: { session: SessionView }) {
  const transcript = transcriptCopy(session);
  return (
    <span className="flex min-w-0 items-center gap-8">
      <CopyText target={sessionCopy(session)}>session {session.short}</CopyText>
      {transcript === null ? (
        <span className="text-13 whitespace-nowrap text-text-2">
          transcript path unknown
        </span>
      ) : (
        <CopyText target={transcript} className="w-transcript">
          <span className="min-w-0 truncate">{transcript.text}</span>
        </CopyText>
      )}
    </span>
  );
}
