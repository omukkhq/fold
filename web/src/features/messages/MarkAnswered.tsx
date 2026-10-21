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
}: {
  slug: string;
  scope: AnsweredScope;
  className?: string;
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
    <span className={cn("inline-flex flex-col items-start gap-2", className)}>
      <Button
        size="sm"
        variant="ghost"
        icon={<CheckCheck aria-hidden="true" size={14} />}
        pending={mark.isPending}
        pendingLabel="Marking…"
        onClick={() => {
          mark.mutate();
        }}
      >
        Mark answered
      </Button>
      {mark.error ? (
        <span role="alert" className="text-meta font-medium text-danger">
          {failureText(mark.error)}
        </span>
      ) : null}
      {note ? <span className="text-meta text-text-muted">{note}</span> : null}
    </span>
  );
}
