import {
  type UseMutationResult,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { CheckCheck } from "lucide-react";
import type { ReactNode } from "react";
import { markAnswered, type AnsweredScope } from "@/api/messages";
import { Button } from "@/components/button/Button";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import { answeredKeys, answeredNote } from "./answered";

type Mark = UseMutationResult<Awaited<ReturnType<typeof markAnswered>>>;

/** The write stays pending until the reads that show the ask are read again. */
function useMark(slug: string, scope: AnsweredScope): Mark {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const closed = await markAnswered(slug, scope);
      await Promise.all(
        answeredKeys(slug, scope).map((queryKey) =>
          queryClient.invalidateQueries({ queryKey }),
        ),
      );
      return closed;
    },
  });
}

function MarkButton({ mark, bar }: { mark: Mark; bar: boolean }) {
  return (
    <Button
      size={bar ? "md" : "sm"}
      variant={bar ? "quiet" : "ghost"}
      icon={<CheckCheck aria-hidden="true" size={bar ? 16 : 14} />}
      pending={mark.isPending}
      pendingLabel="Marking…"
      className={cn(bar && "max-lg:size-44 max-lg:bg-transparent max-lg:px-0")}
      onClick={() => {
        mark.mutate();
      }}
    >
      {bar ? (
        <span className="max-lg:sr-only">Mark answered</span>
      ) : (
        "Mark answered"
      )}
    </Button>
  );
}

/** The refused write and the note of what the mark closed; null while there is neither. */
function markLines(mark: Mark): ReactNode {
  const note = answeredNote(mark.data);
  if (!mark.error && !note) return null;
  return (
    <>
      {mark.error ? (
        <span role="alert" className="text-13 font-medium text-bad">
          {failureText(mark.error)}
        </span>
      ) : null}
      {note ? <span className="text-13 text-text-2">{note}</span> : null}
    </>
  );
}

/** A human marks the open asks of a thread or a task answered, for an answer they gave elsewhere. */
export function MarkAnswered({
  slug,
  scope,
  className,
}: {
  slug: string;
  scope: AnsweredScope;
  className?: string;
}) {
  const mark = useMark(slug, scope);
  return (
    <span className={cn("inline-flex flex-col items-start gap-2", className)}>
      <MarkButton mark={mark} bar={false} />
      {markLines(mark)}
    </span>
  );
}

/**
 * Mark answered for a thread's title bar: a quiet button beside the bar's others, an icon alone below the wide layout.
 * Its error and note come back as `lines`, which the bar draws under itself at full width, so a long error never squeezes the title.
 */
export function useMarkAnsweredBar(
  slug: string,
  scope: AnsweredScope,
): { button: ReactNode; lines: ReactNode; reset: () => void } {
  const mark = useMark(slug, scope);
  return {
    button: <MarkButton mark={mark} bar />,
    lines: markLines(mark),
    reset: mark.reset,
  };
}
