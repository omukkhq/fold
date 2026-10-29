import { PersonList, PersonName } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import {
  ArrowLeft,
  ClipboardList,
  GitPullRequest,
  SquareKanban,
  type LucideIcon,
} from "lucide-react";
import { HrefLink } from "@/app/HrefLink";
import { markThreadRead, type MessageView } from "@/api/messages";
import { sessionQuery } from "@/api/queries";
import { IdLink } from "@/components/id/IdLink";
import type { IdKind } from "@/format/ids";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { useMediaQuery, usePageVisible } from "@/lib/pageState";
import { idHref } from "@/routes/paths";
import { aboutOptions, aboutValue, chatReplyTarget } from "./chatReply";
import {
  chatContexts,
  chatMessages,
  chatName,
  chatParticipants,
  messageContext,
  threadsByMessage,
  type AgentChat,
} from "./chats";
import { replySchema, type ReplyValues } from "./compose";
import { withDays } from "./days";
import { unreadBy } from "./delivery";
import { replyPlaceholder } from "./filters";
import {
  browserSizeObserver,
  followNewest,
  followResizes,
  isFollowing,
  toBottom,
  toMessage,
} from "./follow";
import { useMarkAnsweredBar } from "./MarkAnswered";
import { MessageItem } from "./MessageItem";
import { chatAnchor } from "./openThread";
import { chosenAnswers } from "./question";
import { QuestionDock } from "./QuestionDock";
import {
  cursorAt,
  markableThread,
  openQuestions,
  queueIndex,
  startCursor,
  stepText,
} from "./questionQueue";
import { readsDue, WIDE_LAYOUT } from "./readGate";
import { ReplyBox } from "./ReplyBox";

interface ChatPanelProps {
  slug: string;
  chat: AgentChat;
  /** The thread or message the address names; the chat opens at it. */
  link?: string;
  now: number;
  /** Resolves once the daemon has taken the reply; rejects with its error. */
  onReply: (replyTo: string, body: string) => Promise<void>;
  /** The chat list's address, for the back arrow at phone width. */
  backHref?: string;
  /** The pane is on the screen; a hidden pane marks nothing read. */
  shown?: boolean;
}

const CONTEXT_ICONS: Partial<Record<IdKind, LucideIcon>> = {
  task: SquareKanban,
  proposal: GitPullRequest,
  request: ClipboardList,
};

/** A task, proposal or request the chat is about, as its icon and id. */
function SubjectLink({
  slug,
  kind,
  id,
}: {
  slug: string;
  kind: IdKind;
  id: string;
}) {
  const Icon = CONTEXT_ICONS[kind] ?? SquareKanban;
  return (
    <IdLink
      href={idHref(slug, kind, id)}
      className="inline-flex items-center gap-5 font-medium"
    >
      <Icon aria-hidden="true" size={14} className="shrink-0" />
      {id}
    </IdLink>
  );
}

/** Every message between the signed-in human and one agent, across its threads, with the asks that wait for an answer docked above the reply box. */
export function ChatPanel({
  slug,
  chat,
  link,
  now,
  onReply,
  backHref,
  shown = true,
}: ChatPanelProps) {
  const you = useQuery(sessionQuery()).data?.human ?? null;
  const messages = chatMessages(chat);
  const threadOf = threadsByMessage(chat);
  const newest = messages.at(-1)?.id ?? null;
  const [anchor] = useState(() => chatAnchor(chat, link, you));
  const queue = openQuestions(chat, you);
  // Without a link the dock starts at the oldest question, even when the newest message is a question.
  const [cursor, setCursor] = useState(() =>
    startCursor(queue, link === undefined ? null : anchor),
  );
  const index = queueIndex(queue, cursor);
  const current = queue[index] ?? null;
  const step = stepText(index, queue.length);
  const [about, setAbout] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const target = chatReplyTarget(chat, you, current, about, picked);
  const options = aboutOptions(chat, current, step);
  const markable = markableThread(chat, current);
  const mark = useMarkAnsweredBar(slug, { thread: markable?.id ?? "" });
  const visible = usePageVisible();
  const wide = useMediaQuery(WIDE_LAYOUT);

  // The `message.acknowledged` event of each call refreshes the chat.
  const marked = useRef(new Map<string, string>());
  useEffect(() => {
    const unread = chat.threads.flatMap((t) => {
      const id = unreadBy(t.messages, you);
      return id === null ? [] : [{ thread: t.id, unread: id }];
    });
    for (const call of readsDue(unread, shown, visible, marked.current)) {
      marked.current.set(call.thread, call.unread);
      markThreadRead(slug, call.thread).catch(() => undefined);
    }
  }, [slug, chat.threads, you, shown, visible]);

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
  const anchored = useRef(false);
  // A chat opens at the linked message, else at its newest; a new message scrolls into view only for a reader at the bottom.
  useLayoutEffect(() => {
    const box = scroller.current;
    if (!box) return;
    if (!anchored.current) {
      anchored.current = true;
      const at =
        anchor === null || anchor === newest
          ? null
          : box.querySelector(`[data-message="${anchor}"]`);
      if (at instanceof HTMLElement) {
        toMessage(box, at.offsetTop);
        following.current = false;
        return;
      }
    }
    if (following.current) {
      toBottom(box);
    }
  }, [anchor, newest, messages.length]);
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

  const writeOwn = (question: MessageView) => {
    setPicked(question.id);
    setFocus("body");
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

  const name = chatName(chat);
  const chosen = new Set(chat.threads.flatMap((t) => [...chosenAnswers(t)]));
  const replyThread = target?.thread ?? chat.threads[0];
  return (
    <section
      aria-label={name}
      className="flex min-h-0 min-w-0 flex-1 flex-col bg-bg"
    >
      <header className="flex shrink-0 items-start gap-6 bg-surface py-8 pr-12 pl-6 lg:gap-16 lg:bg-transparent lg:px-28 lg:pt-20 lg:pb-12">
        {backHref === undefined ? null : (
          <HrefLink
            href={backHref}
            aria-label="Back to chats"
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
            {chat.agent === null ? name : <PersonName name={chat.agent} />}
          </h2>
          <p className="flex flex-wrap items-center gap-x-10 gap-y-2 text-13 lg:gap-x-14 lg:gap-y-4 lg:text-14">
            {chatContexts(chat).map((context) => (
              <SubjectLink
                key={context.id}
                slug={slug}
                kind={context.kind}
                id={context.id}
              />
            ))}
            <span className="text-text">
              <PersonList
                names={chatParticipants(chat)}
                you={you}
                youWord
                nameClassName="text-13 lg:text-14"
              />
            </span>
          </p>
        </div>
        {markable === null ? null : (
          <div className="flex shrink-0 items-start gap-8">{mark.button}</div>
        )}
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
          "relative flex min-h-0 flex-1 flex-col overflow-y-auto px-16 lg:px-28",
          insetFocusRing,
        )}
      >
        <ol
          ref={content}
          className="flex w-full max-w-thread-column flex-col gap-18 pt-14 pb-12 lg:gap-24 lg:pt-6 lg:pb-20"
        >
          {withDays(messages, now).map((item) => {
            if (item.kind === "day") {
              return (
                <li
                  key={item.key}
                  className="font-title text-13 leading-tight font-semibold text-text-2"
                >
                  {item.label}
                </li>
              );
            }
            const thread = threadOf.get(item.message.id);
            if (thread === undefined) return null;
            const queued = queue.findIndex(
              (q) => q.message.id === item.message.id,
            );
            return (
              <MessageItem
                key={item.key}
                slug={slug}
                thread={thread}
                message={item.message}
                now={now}
                you={you}
                chosen={chosen.has(item.message.id)}
                context={messageContext(item.message, thread)}
                docked={queued < 0 ? null : stepText(queued, queue.length)}
                onDock={() => {
                  setCursor(cursorAt(queue, queued));
                }}
                onReply={reply}
                onWriteOwn={writeOwn}
              />
            );
          })}
        </ol>
      </div>

      <QuestionDock
        slug={slug}
        queue={queue}
        index={index}
        step={step}
        you={you}
        onStep={(next) => {
          setCursor(cursorAt(queue, next));
        }}
        onReply={reply}
        onWriteOwn={writeOwn}
      />

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
            placeholder={
              replyThread === undefined
                ? "Reply…"
                : replyPlaceholder(replyThread, wide)
            }
            pending={isSubmitting}
            answering={target?.answers?.sender ?? null}
            about={
              options.length < 2 ? null : (
                <label className="flex min-w-0 flex-1 items-center gap-6 sm:flex-none">
                  <span className="shrink-0">About</span>
                  <select
                    value={aboutValue(chat, current, about) ?? ""}
                    disabled={isSubmitting}
                    onChange={(event) => {
                      setAbout(event.target.value);
                      setPicked(null);
                    }}
                    className={cn(
                      "min-w-0 flex-1 truncate rounded-s bg-fill px-8 py-3 text-13 text-text max-sm:min-h-44 sm:flex-none",
                      focusRing,
                    )}
                  >
                    {options.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
              )
            }
            invalid={errors.body?.message ?? null}
            failure={errors.root?.message ?? null}
            onSubmit={(event) => void handleSubmit(send)(event)}
          />
        )}
      />
    </section>
  );
}
