import { PersonList } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import {
  ArrowLeft,
  GitPullRequest,
  SquareKanban,
  type LucideIcon,
} from "lucide-react";
import { HrefLink } from "@/app/HrefLink";
import { markThreadRead, type ThreadView } from "@/api/messages";
import { sessionQuery } from "@/api/queries";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { Id } from "@/components/id/Id";
import { IdLink } from "@/components/id/IdLink";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { useMediaQuery, usePageVisible } from "@/lib/pageState";
import { replySchema, type ReplyValues } from "./compose";
import { withDays } from "./days";
import {
  browserSizeObserver,
  followNewest,
  followResizes,
  isFollowing,
  toBottom,
} from "./follow";
import { unreadBy } from "./delivery";
import { replyPlaceholder } from "./filters";
import { useMarkAnsweredBar } from "./MarkAnswered";
import { threadCanMarkAnswered } from "./answered";
import { MessageItem } from "./MessageItem";
import { chosenAnswers } from "./question";
import { ReplyBox } from "./ReplyBox";
import { readDue, WIDE_LAYOUT } from "./readGate";
import { replyTarget } from "./replyTarget";
import { threadTitle } from "./title";
import { projectPath } from "@/routes/paths";

interface ThreadPanelProps {
  slug: string;
  thread: ThreadView;
  now: number;
  /** Resolves once the daemon has taken the reply; rejects with its error. */
  onReply: (replyTo: string, body: string) => Promise<void>;
  /** The thread list's address, for the back arrow at phone width. */
  backHref?: string;
  /** The pane is on the screen; a hidden pane marks nothing read. */
  shown?: boolean;
}

/** A task or proposal the thread is about, as its icon and id. */
function SubjectLink({
  href,
  icon: Icon,
  id,
}: {
  href: string;
  icon: LucideIcon;
  id: string;
}) {
  return (
    <IdLink href={href} className="inline-flex items-center gap-5 font-medium">
      <Icon aria-hidden="true" size={14} className="shrink-0" />
      {id}
    </IdLink>
  );
}

export function ThreadPanel({
  slug,
  thread,
  now,
  onReply,
  backHref,
  shown = true,
}: ThreadPanelProps) {
  const chosen = chosenAnswers(thread);
  const you = useQuery(sessionQuery()).data?.human ?? null;
  const mark = useMarkAnsweredBar(slug, { thread: thread.id });
  const [picked, setPicked] = useState<string | null>(null);
  const target = replyTarget(thread, you, picked);
  const unread = unreadBy(thread.messages, you);
  const visible = usePageVisible();
  const wide = useMediaQuery(WIDE_LAYOUT);
  // The `message.acknowledged` event of the call refreshes the thread.
  const marked = useRef<string | null>(null);
  useEffect(() => {
    const due = readDue({ unread, shown, visible, marked: marked.current });
    if (due === null) return;
    marked.current = due;
    markThreadRead(slug, thread.id).catch(() => undefined);
  }, [slug, thread.id, unread, shown, visible]);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
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
    if (!target) {
      return;
    }
    try {
      await reply(target.replyTo, body.trim());
      reset();
      setPicked(null);
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  };

  const title = threadTitle(thread);
  return (
    <section
      aria-label={title}
      className="flex min-h-0 min-w-0 flex-1 flex-col bg-bg"
    >
      <header className="flex shrink-0 items-start gap-6 bg-surface py-8 pr-12 pl-6 lg:gap-16 lg:bg-transparent lg:px-28 lg:pt-20 lg:pb-12">
        {backHref === undefined ? null : (
          <HrefLink
            href={backHref}
            aria-label="Back to threads"
            className={cn(
              "grid size-44 shrink-0 place-items-center rounded-m text-text hover:bg-sunken lg:hidden",
              focusRing,
            )}
          >
            <ArrowLeft aria-hidden="true" size={20} />
          </HrefLink>
        )}
        <div className="flex min-w-0 flex-1 flex-col gap-4 max-lg:py-4 lg:gap-6">
          <h2 className="line-clamp-2 font-title text-16 leading-tight font-semibold text-text lg:line-clamp-none lg:max-w-measure lg:text-21">
            {title}
          </h2>
          <p className="flex flex-wrap items-center gap-x-10 gap-y-2 text-13 lg:gap-x-14 lg:gap-y-4 lg:text-14">
            {thread.task ? (
              <SubjectLink
                href={projectPath(slug, "t", thread.task)}
                icon={SquareKanban}
                id={thread.task}
              />
            ) : null}
            {thread.proposal ? (
              <SubjectLink
                href={projectPath(slug, "proposals", thread.proposal)}
                icon={GitPullRequest}
                id={thread.proposal}
              />
            ) : null}
            <span className="text-text">
              <PersonList
                names={thread.participants}
                you={you}
                youWord
                nameClassName="text-13 lg:text-14"
              />
            </span>
            <Id className="hidden text-13 text-text-2 lg:inline">
              {thread.id}
            </Id>
          </p>
        </div>
        <div className="flex shrink-0 items-start gap-8">
          {threadCanMarkAnswered(thread) ? mark.button : null}
          {thread.task ? (
            <HrefLink
              href={projectPath(slug, "t", thread.task)}
              className={cn(
                buttonClasses("quiet", "md"),
                "max-lg:size-44 max-lg:bg-transparent max-lg:p-0",
                focusRing,
              )}
            >
              <span className={buttonIconClass("quiet")}>
                <SquareKanban aria-hidden="true" size={16} />
              </span>
              <span className="max-lg:sr-only">Open {thread.task}</span>
            </HrefLink>
          ) : null}
        </div>
      </header>
      {mark.lines === null ? null : (
        <div className="flex shrink-0 flex-col gap-2 bg-surface px-12 pb-8 lg:bg-transparent lg:px-28">
          {mark.lines}
        </div>
      )}

      <div
        ref={scroller}
        onScroll={(event) => {
          following.current = isFollowing(event.currentTarget);
        }}
        role="region"
        aria-label="Messages"
        tabIndex={0}
        className={cn(
          "flex min-h-0 flex-1 flex-col overflow-y-auto px-16 lg:px-28",
          insetFocusRing,
        )}
      >
        <ol
          ref={content}
          className="flex w-full max-w-thread-column flex-col gap-18 pt-14 pb-12 lg:gap-24 lg:pt-6 lg:pb-20"
        >
          {withDays(thread.messages, now).map((item) =>
            item.kind === "day" ? (
              <li
                key={item.key}
                className="font-title text-13 leading-tight font-semibold text-text-2"
              >
                {item.label}
              </li>
            ) : (
              <MessageItem
                key={item.key}
                slug={slug}
                thread={thread}
                message={item.message}
                now={now}
                you={you}
                chosen={chosen.has(item.message.id)}
                onReply={reply}
                onWriteOwn={(question) => {
                  setPicked(question.id);
                  setFocus("body");
                }}
              />
            ),
          )}
        </ol>
      </div>

      <Controller
        control={control}
        name="body"
        render={({ field }) => (
          <ReplyBox
            slug={slug}
            value={field.value}
            onValueChange={field.onChange}
            onBlur={field.onBlur}
            boxRef={field.ref}
            placeholder={replyPlaceholder(thread, wide)}
            pending={isSubmitting}
            answering={target?.answers?.sender ?? null}
            invalid={errors.body?.message ?? null}
            failure={errors.root?.message ?? null}
            onSubmit={(event) => void handleSubmit(send)(event)}
          />
        )}
      />
    </section>
  );
}
