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
import { commentNote, composerTitle, type CommentThread } from "./threadsModel";

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
      className="flex flex-col gap-10"
    >
      {thread.comments.map((entry) => (
        <li key={entry.comment.id}>
          <article
            className={cn(
              "flex flex-col gap-2",
              thread.outdated && !entry.reply && "opacity-80",
            )}
          >
            <p className="text-13 text-text-2">
              <PersonName name={entry.comment.author} /> ·{" "}
              <span>{commentNote(thread, entry)}</span> ·{" "}
              {formatAsOfMinute(now, entry.comment.created_at)}
            </p>
            <MentionText
              text={entry.comment.body}
              slug={slug}
              className="max-w-measure text-15 leading-normal"
            />
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
      <p className="flex items-center gap-8 text-13 leading-tight font-semibold text-accent">
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
        <p className="text-13 font-medium text-bad">{errors.body.message}</p>
      ) : null}
      {errors.root ? (
        <p role="alert" className="text-13 font-medium text-bad">
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
        <span className="text-13 text-text-2">
          Shift-click line numbers to select a range
        </span>
      </div>
    </form>
  );
}
