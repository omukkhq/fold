import { PersonList, PersonName } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import { Controller, useForm } from "react-hook-form";
import { ArrowRight, Send, SquareKanban } from "lucide-react";
import { HrefLink } from "@/app/HrefLink";
import type { ThreadView } from "@/api/messages";
import { Button } from "@/components/button/Button";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { failureText } from "@/format/failure";
import { formatAsOf } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { replySchema, type ReplyValues } from "./compose";
import { deliveryLines, recipientNames } from "./delivery";
import { replyTargets } from "./filters";
import { MessageBody } from "./MessageBody";
import { threadTitle } from "./title";

interface ThreadPanelProps {
  slug: string;
  thread: ThreadView;
  now: number;
  /** Resolves once the daemon has taken the reply; rejects with its error. */
  onReply: (replyTo: string, body: string) => Promise<void>;
}

export function ThreadPanel({ slug, thread, now, onReply }: ThreadPanelProps) {
  const last = thread.messages.at(-1);
  const targets = replyTargets(thread);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<ReplyValues>({
    resolver: zodResolver(replySchema),
    defaultValues: { body: "" },
  });

  const submit = handleSubmit(async ({ body }) => {
    if (!last) {
      return;
    }
    try {
      await onReply(last.id, body.trim());
      reset();
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  });

  return (
    <section
      aria-label={threadTitle(thread)}
      className="flex min-w-0 flex-1 flex-col rounded-l bg-surface inset-ring inset-ring-border-subtle"
    >
      <header className="flex items-center gap-10 border-b border-border-subtle px-18 py-14">
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <h2 className="text-section leading-snug font-semibold text-text">
            {threadTitle(thread)}
          </h2>
          <p className="font-mono text-meta text-text-muted">
            thread {thread.id}
            {thread.task ? ` · linked to ${thread.task}` : ""}
            {thread.proposal ? ` · ${thread.proposal}` : ""} ·{" "}
            <PersonList names={thread.participants} />
          </p>
        </div>
        {thread.task ? (
          <HrefLink
            href={`/p/${slug}/t/${thread.task}`}
            className={cn(buttonClasses("secondary", "md"), focusRing)}
          >
            <span className={buttonIconClass("secondary")}>
              <SquareKanban aria-hidden="true" size={15} />
            </span>
            Open {thread.task}
          </HrefLink>
        ) : null}
      </header>

      <ol className="flex flex-1 flex-col gap-12 p-18">
        {thread.messages.map((message) => (
          <li key={message.id} className="flex flex-col gap-4">
            <article className="flex flex-col gap-4 rounded-m bg-surface px-12 py-10 inset-ring inset-ring-border-subtle">
              <p className="flex items-center gap-8 font-mono text-caption text-text-muted">
                <PersonName name={message.sender} avatarClassName="font-ui" />
                <ArrowRight
                  aria-label="to"
                  role="img"
                  size={12}
                  className="shrink-0 text-text-faint"
                />
                <span className="min-w-0 truncate">
                  {recipientNames(message)}
                </span>
                <span className="ml-auto font-ui text-meta text-text-faint">
                  {formatAsOf(now, message.sent_at)}
                </span>
              </p>
              <MessageBody body={message.body} slug={slug} />
            </article>
            <ul className="flex flex-col gap-2">
              {deliveryLines(message, now).map((line) => (
                <li key={line} className="text-meta text-text-faint">
                  {line}
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ol>

      <form
        onSubmit={(event) => void submit(event)}
        noValidate
        className="flex flex-col gap-6 border-t border-border-subtle px-18 py-12"
      >
        <div className="flex items-center gap-8">
          <Controller
            control={control}
            name="body"
            render={({ field }) => (
              <MentionTextarea
                slug={slug}
                rows={1}
                aria-label="Reply"
                placeholder={
                  targets.length > 0
                    ? `Reply to ${targets.join(", ")}…`
                    : "Reply…"
                }
                readOnly={isSubmitting}
                aria-invalid={errors.body ? true : undefined}
                className="resize-none py-9"
                ref={field.ref}
                value={field.value}
                onValueChange={field.onChange}
                onBlur={field.onBlur}
              />
            )}
          />
          <Button
            type="submit"
            variant="primary"
            pending={isSubmitting}
            icon={<Send aria-hidden="true" size={13} />}
          >
            Send
          </Button>
        </div>
        {errors.body ? (
          <p className="text-meta font-medium text-danger">
            {errors.body.message}
          </p>
        ) : null}
        {errors.root ? (
          <p role="alert" className="text-meta font-medium text-danger">
            The reply was not sent. {errors.root.message}
          </p>
        ) : null}
      </form>
    </section>
  );
}
