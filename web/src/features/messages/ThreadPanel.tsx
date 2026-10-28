import { PersonList } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { ArrowLeft, GitPullRequest, SquareKanban } from "lucide-react";
import { HrefLink } from "@/app/HrefLink";
import { markThreadRead, type ThreadView } from "@/api/messages";
import { sessionQuery } from "@/api/queries";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { usePageVisible } from "@/lib/pageState";
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
import { MarkAnswered } from "./MarkAnswered";
import { threadCanMarkAnswered } from "./answered";
import { MessageItem } from "./MessageItem";
import { chosenAnswers } from "./question";
import { ReplyBox } from "./ReplyBox";
import { readDue } from "./readGate";
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

const CHIP =
  "hidden items-center gap-6 rounded-pill bg-surface px-10 py-3 text-13 text-text-2 inset-ring inset-ring-line hover:text-text lg:inline-flex";

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
  const [picked, setPicked] = useState<string | null>(null);
  const target = replyTarget(thread, you, picked);
  const unread = unreadBy(thread.messages, you);
  const visible = usePageVisible();
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
      <header className="flex shrink-0 items-center gap-8 border-b border-line py-6 pr-8 pl-4 lg:items-start lg:gap-16 lg:px-28 lg:py-16">
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
        <div className="flex min-w-0 flex-1 flex-col gap-2 lg:gap-8">
          <h2 className="truncate text-16 leading-tight font-semibold text-text lg:text-16 lg:whitespace-normal">
            {title}
          </h2>
          <p className="flex flex-wrap items-center gap-x-8 gap-y-4 text-13 text-text-2">
            {thread.task ? (
              <HrefLink
                href={projectPath(slug, "t", thread.task)}
                className={cn(CHIP, focusRing)}
              >
                <SquareKanban aria-hidden="true" size={13} />
                <span className="font-mono text-13">{thread.task}</span>
              </HrefLink>
            ) : null}
            {thread.proposal ? (
              <HrefLink
                href={projectPath(slug, "proposals", thread.proposal)}
                className={cn(CHIP, focusRing)}
              >
                <GitPullRequest aria-hidden="true" size={13} />
                <span className="font-mono text-13">{thread.proposal}</span>
              </HrefLink>
            ) : null}
            <span className="hidden font-mono lg:inline">{thread.id}</span>
            <span aria-hidden="true" className="hidden lg:inline">
              ·
            </span>
            <PersonList
              names={thread.participants}
              you={you}
              youWord
              nameClassName="text-13 lg:text-14"
            />
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-8">
          {threadCanMarkAnswered(thread) ? (
            <MarkAnswered slug={slug} scope={{ thread: thread.id }} />
          ) : null}
          {thread.task ? (
            <HrefLink
              href={projectPath(slug, "t", thread.task)}
              className={cn(
                buttonClasses("secondary", "md"),
                "max-lg:size-44 max-lg:bg-transparent max-lg:p-0 max-lg:inset-ring-0",
                focusRing,
              )}
            >
              <span className={cn(buttonIconClass("secondary"), "lg:hidden")}>
                <SquareKanban aria-hidden="true" size={18} />
              </span>
              <span className="max-lg:sr-only">Open {thread.task}</span>
            </HrefLink>
          ) : null}
        </div>
      </header>

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
          className="mt-auto flex w-full max-w-thread-column flex-col gap-18 pt-14 pb-8 lg:gap-22 lg:pt-20"
        >
          {withDays(thread.messages, now).map((item) =>
            item.kind === "day" ? (
              <li key={item.key} className="flex items-center gap-12">
                <span aria-hidden="true" className="h-px flex-1 bg-line" />
                <span className="text-13 font-medium text-text-2">
                  {item.label}
                </span>
                <span aria-hidden="true" className="h-px flex-1 bg-line" />
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
            placeholder={replyPlaceholder(thread)}
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
