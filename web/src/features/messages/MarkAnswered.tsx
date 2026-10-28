import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CheckCheck } from "lucide-react";
import { markAnswered, type AnsweredScope } from "@/api/messages";
import { Button } from "@/components/button/Button";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import { answeredKeys, answeredNote } from "./answered";

/** A human marks the open asks of a thread or a task answered, for an answer they gave elsewhere. The write stays pending until the reads that show the ask are read again. */
export function MarkAnswered({
  slug,
  scope,
  className,
  bar = false,
}: {
  slug: string;
  scope: AnsweredScope;
  className?: string;
  /** The button sits in a thread's title bar: a quiet button beside the others, an icon alone below the wide layout. */
  bar?: boolean;
}) {
  const queryClient = useQueryClient();
  const mark = useMutation({
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
  const note = answeredNote(mark.data);
  return (
    <span
      className={cn(
        "inline-flex flex-col gap-2",
        bar ? "items-end" : "items-start",
        className,
      )}
    >
      <Button
        size={bar ? "md" : "sm"}
        variant={bar ? "quiet" : "ghost"}
        icon={<CheckCheck aria-hidden="true" size={bar ? 16 : 14} />}
        pending={mark.isPending}
        pendingLabel="Marking…"
        className={cn(
          bar && "max-lg:size-44 max-lg:bg-transparent max-lg:px-0",
        )}
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
      {mark.error ? (
        <span role="alert" className="text-13 font-medium text-bad">
          {failureText(mark.error)}
        </span>
      ) : null}
      {note ? <span className="text-13 text-text-2">{note}</span> : null}
    </span>
  );
}
