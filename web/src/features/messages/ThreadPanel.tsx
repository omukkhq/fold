import { PersonList, PersonName } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useLayoutEffect, useRef } from "react";
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
import { focusRing, insetFocusRing } from "@/lib/focus";
import { replySchema, type ReplyValues } from "./compose";
import {
  browserSizeObserver,
  followNewest,
  followResizes,
  isFollowing,
  toBottom,
} from "./follow";
import { deliveryLines, recipientActors } from "./delivery";
import { replyPlaceholder } from "./filters";
import { MarkAnswered } from "./MarkAnswered";
import { MessageBody } from "./MessageBody";
import { chosenAnswers, isQuestion } from "./question";
import {
  QuestionBadge,
  QuestionChoices,
  QuestionHeader,
} from "./QuestionChoices";
import { threadTitle } from "./title";
import { projectPath } from "@/routes/paths";

interface ThreadPanelProps {
  slug: string;
  thread: ThreadView;
  now: number;
  /** Resolves once the daemon has taken the reply; rejects with its error. */
  onReply: (replyTo: string, body: string) => Promise<void>;
}

export function ThreadPanel({ slug, thread, now, onReply }: ThreadPanelProps) {
  const last = thread.messages.at(-1);
  const chosen = chosenAnswers(thread);
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

  const scroller = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLOListElement>(null);
  const following = useRef(true);
  // A thread opens at its newest message; a new message scrolls into view only for a reader at the bottom.
  useLayoutEffect(() => {
    const box = scroller.current;
    if (box && following.current) {
      toBottom(box);
    }
  }, [thread.messages.length]);
  useEffect(() => {
    const box = scroller.current;
    const list = content.current;
    if (!box || !list) {
      return undefined;
    }
    return followResizes(box, [box, list], following, browserSizeObserver);
  }, []);

  // The new message may render before or after the send resolves; the layout effect covers the later case.
  const reply = async (replyTo: string, body: string) => {
    await onReply(replyTo, body);
    followNewest(scroller.current, following);
  };

  const send = async ({ body }: ReplyValues) => {
    if (!last) {
      return;
    }
    try {
      await reply(last.id, body.trim());
      reset();
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  };

  return (
    <section
      aria-label={threadTitle(thread)}
      className="flex min-h-0 min-w-0 flex-1 flex-col rounded-l bg-surface inset-ring inset-ring-border-subtle"
    >
      <header className="flex shrink-0 items-center gap-10 border-b border-border-subtle px-18 py-14">
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
        {thread.awaiting_human ? (
          <MarkAnswered slug={slug} scope={{ thread: thread.id }} />
        ) : null}
        {thread.task ? (
          <HrefLink
            href={projectPath(slug, "t", thread.task)}
            className={cn(buttonClasses("secondary", "md"), focusRing)}
          >
            <span className={buttonIconClass("secondary")}>
              <SquareKanban aria-hidden="true" size={15} />
            </span>
            Open {thread.task}
          </HrefLink>
        ) : null}
      </header>

      <div
        ref={scroller}
        onScroll={(event) => {
          following.current = isFollowing(event.currentTarget);
        }}
        role="region"
        aria-label="Messages"
        tabIndex={0}
        className={cn("min-h-0 flex-1 overflow-y-auto", insetFocusRing)}
      >
        <ol ref={content} className="flex flex-col gap-12 p-18">
          {thread.messages.map((message) => (
            <li key={message.id} className="flex flex-col gap-4">
              {isQuestion(message) ? (
                <QuestionBadge thread={thread} question={message} />
              ) : null}
              {isQuestion(message) && message.header !== null ? (
                <QuestionHeader header={message.header} />
              ) : null}
              <article className="flex flex-col gap-4 rounded-m bg-surface px-12 py-10 inset-ring inset-ring-border-subtle">
                <p className="flex items-center gap-8 font-mono text-caption text-text-muted">
                  <PersonName name={message.sender} slot="messageHead" />
                  <ArrowRight
                    aria-label="to"
                    role="img"
                    size={12}
                    className="shrink-0 text-text-faint"
                  />
                  <PersonList
                    names={recipientActors(message)}
                    nameClassName="text-bar"
                  />
                  <span className="ml-auto font-ui text-meta text-text-faint">
                    {formatAsOf(now, message.sent_at)}
                  </span>
                </p>
                {chosen.has(message.id) ? (
                  <div className="flex flex-col gap-4">
                    <p className="text-body font-semibold text-text">
                      {message.body.trim()}
                    </p>
                    <p className="text-code text-text-muted">
                      Chosen option of the question above.
                    </p>
                  </div>
                ) : (
                  <MessageBody body={message.body} slug={slug} />
                )}
                {isQuestion(message) ? (
                  <QuestionChoices
                    thread={thread}
                    question={message}
                    onReply={reply}
                  />
                ) : null}
              </article>
              <ul className="flex flex-col gap-2">
                {deliveryLines(message, now).map((line) => (
                  <li
                    key={line.actor}
                    className="flex items-center gap-6 text-meta text-text-faint"
                  >
                    {line.lead}
                    <PersonName name={line.actor} className="text-bar" />
                    {line.at === null ? null : ` · ${line.at}`}
                    {line.note === null ? null : ` · ${line.note}`}
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ol>
      </div>

      <form
        onSubmit={(event) => void handleSubmit(send)(event)}
        noValidate
        className="flex shrink-0 flex-col gap-6 border-t border-border-subtle px-18 py-12"
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
                placeholder={replyPlaceholder(thread)}
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
            pendingLabel="Sending…"
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
