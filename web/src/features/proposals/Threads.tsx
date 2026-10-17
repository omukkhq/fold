import { PersonName } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import { MessageSquarePlus, TextSelect } from "lucide-react";
import { Controller, useForm } from "react-hook-form";
import { Button } from "@/components/button/Button";
import type { DiffSelection } from "@/features/diff/selection";
import { MentionText } from "@/mentions/MentionText";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { commentSchema, type CommentValues } from "./compose";
import { commentNote, composerTitle, type CommentThread } from "./threads";

export function ThreadBox({
  slug,
  thread,
  now,
}: {
  slug: string;
  thread: CommentThread;
  now: number;
}) {
  return (
    <ol
      aria-label={`Thread ${thread.id}`}
      data-outdated={thread.outdated || undefined}
      className="flex flex-col gap-8"
    >
      {thread.comments.map((entry) => (
        <li key={entry.comment.id}>
          <article
            className={cn(
              "flex flex-col gap-4 rounded-m bg-surface px-12 py-10 inset-ring inset-ring-border-subtle",
              thread.outdated && !entry.reply && "opacity-80",
            )}
          >
            <p className="flex items-center gap-8 font-mono text-caption text-text-muted">
              <PersonName name={entry.comment.author} className="text-meta" />
              <span className="min-w-0 truncate">
                {commentNote(thread, entry)}
              </span>
              <span className="ml-auto font-ui text-meta text-text-faint">
                {formatAsOfMinute(now, entry.comment.created_at)}
              </span>
            </p>
            <MentionText text={entry.comment.body} slug={slug} />
          </article>
        </li>
      ))}
    </ol>
  );
}

interface ComposerProps {
  slug: string;
  selection: DiffSelection;
  human: string;
  /** Resolves once the daemon has stored the comment; rejects with its error. */
  onSubmit: (body: string) => Promise<void>;
  onCancel: () => void;
  /** The sentence for a failure, from the screen's error mapping. */
  failureText: (error: unknown) => string;
}

export function CommentComposer({
  slug,
  selection,
  human,
  onSubmit,
  onCancel,
  failureText,
}: ComposerProps) {
  const {
    control,
    handleSubmit,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<CommentValues>({
    resolver: zodResolver(commentSchema),
    defaultValues: { body: "" },
  });
  const title = composerTitle(selection, human);

  const submit = handleSubmit(async ({ body }) => {
    try {
      await onSubmit(body.trim());
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  });

  return (
    <form
      aria-label={title}
      noValidate
      onSubmit={(event) => void submit(event)}
      className="flex flex-col gap-8"
    >
      <p className="flex items-center gap-8 text-code leading-snug font-semibold text-primary-text">
        <TextSelect aria-hidden="true" size={14} />
        {title}
      </p>
      <Controller
        control={control}
        name="body"
        render={({ field }) => (
          <MentionTextarea
            slug={slug}
            rows={2}
            aria-label="Comment"
            autoFocus
            readOnly={isSubmitting}
            aria-invalid={errors.body ? true : undefined}
            className="resize-y py-9"
            ref={field.ref}
            value={field.value}
            onValueChange={field.onChange}
            onBlur={field.onBlur}
          />
        )}
      />
      {errors.body ? (
        <p className="text-meta font-medium text-danger">
          {errors.body.message}
        </p>
      ) : null}
      {errors.root ? (
        <p role="alert" className="text-meta font-medium text-danger">
          The comment was not saved. {errors.root.message}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-8">
        <Button
          type="submit"
          variant="primary"
          pending={isSubmitting}
          pendingLabel="Adding to review…"
          icon={<MessageSquarePlus aria-hidden="true" size={14} />}
        >
          Add to review
        </Button>
        <Button variant="ghost" disabled={isSubmitting} onClick={onCancel}>
          Cancel
        </Button>
        <span className="text-meta text-text-muted">
          Shift-click line numbers to select a range
        </span>
      </div>
    </form>
  );
}
