import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { shortCommit } from "@/format/git";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { copyOutcome, type CopyState } from "./commitCopy";
import { Mono } from "./Mono";

/** The short Git commit id in the mono face. It copies the full id when clicked, unless `copy` is false, for a place inside another button. A commit that is not written yet shows nothing. */
export function CommitId({
  commit,
  copy = true,
  className,
}: {
  commit: string | null | undefined;
  copy?: boolean;
  className?: string;
}) {
  const [state, setState] = useState<CopyState>("idle");
  if (commit == null) {
    return null;
  }
  const short = shortCommit(commit);
  if (!copy) {
    return (
      <Mono title={commit} className={className}>
        {short}
      </Mono>
    );
  }
  return (
    <button
      type="button"
      title={commit}
      aria-label={`Copy commit ${short}`}
      onClick={() => {
        void copyOutcome(commit, navigator.clipboard).then(setState);
      }}
      onBlur={() => {
        setState("idle");
      }}
      className={cn(
        "inline-flex items-center gap-4 rounded-s font-mono text-meta text-text-muted hover:text-text",
        focusRing,
        className,
      )}
    >
      {short}
      {state === "copied" ? (
        <Check aria-hidden size={12} />
      ) : (
        <Copy aria-hidden size={12} className="text-text-faint" />
      )}
      <span role="status" className="sr-only">
        {state === "copied" ? "Copied" : state === "failed" ? "Not copied" : ""}
      </span>
    </button>
  );
}
