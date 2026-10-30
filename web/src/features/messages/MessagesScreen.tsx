import {
  useMutation,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import {
  ArrowLeft,
  MessageSquarePlus,
  MessagesSquare,
  Search,
  SearchX,
} from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import { answerAsk, sendMessage, type NewMessage } from "@/api/messages";
import { olderNotListed } from "@/api/pages";
import {
  messagesQuery,
  needsYouQuery,
  sessionQuery,
  threadQuery,
} from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { buttonClasses } from "@/components/button/buttonStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useMediaQuery } from "@/lib/pageState";
import { IconButton } from "@/components/button/Button";
import { Tooltip } from "@/components/overlay/Tooltip";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { ChatList } from "./ChatList";
import { answeredKeys } from "./answered";
import { ChatPanel } from "./ChatPanel";
import { groupChats } from "./chats";
import {
  chatCount,
  filterOptions,
  filterThreads,
  findThread,
  searchThreads,
  showOptions,
  withShow,
  type ThreadFilter,
} from "./filters";
import { FeedbackDialog } from "./FeedbackDialog";
import {
  isMissingThread,
  missingChatTitle,
  openChat,
  visibleChats,
} from "./openThread";
import { messagesHref, type MessagesSearch } from "./search";
import { openAskIds } from "./sets";
import { ThreadFilterMenu } from "./ThreadFilterMenu";
import { paneShown, WIDE_LAYOUT } from "./readGate";

interface MessagesScreenProps {
  slug: string;
  search: MessagesSearch;
}

export function MessagesScreen({ slug, search }: MessagesScreenProps) {
  const { data: listing, dataUpdatedAt } = useSuspenseQuery(
    messagesQuery(slug),
  );
  const data = listing.items;
  const router = useRouter();
  const queryClient = useQueryClient();
  const you = useQuery(sessionQuery()).data?.human ?? null;
  const [feedback, setFeedback] = useState(false);
  const [query, setQuery] = useState("");
  const send = useMutation({
    mutationFn: (input: NewMessage) => sendMessage(slug, input),
    // The daemon's answer decides whether a thread still awaits a human, so the list is refetched rather than patched.
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: keys.messages(slug) }),
  });
  // The card stays pending until the reads that show the set are read again, so the set leaves only once the daemon closed it.
  const answer = useMutation({
    mutationFn: async (input: { ask: string; answers: string[] }) => {
      await answerAsk(slug, input.ask, input.answers);
      await Promise.all(
        answeredKeys(slug, { ask: input.ask }).map((queryKey) =>
          queryClient.invalidateQueries({ queryKey }),
        ),
      );
    },
  });
  const openAsks = openAskIds(useQuery(needsYouQuery(slug)).data ?? [], you);

  const filter: ThreadFilter = {
    agent: search.agent,
    task: search.task,
    proposal: search.proposal,
    waiting: search.waiting,
    questions: search.questions,
    mine: search.mine,
  };
  const missing =
    search.thread !== undefined && findThread(data, search.thread) === null
      ? search.thread
      : null;
  // A thread the listing did not reach is read on its own and joins the chat of its agent.
  const unlisted = useQuery({
    ...threadQuery(slug, missing ?? ""),
    enabled: missing !== null,
  });
  const all =
    missing !== null && unlisted.data ? [...data, unlisted.data] : data;
  const chats = groupChats(all);
  const kept = filterThreads(all, filter, you);
  const visible = visibleChats(chats, kept);
  const shown = visibleChats(visible, searchThreads(kept, query));
  const target = openChat(all, chats, visible, search);
  const open = target.kind === "listed" ? target.chat : null;
  const requested = search.thread ?? search.chat;
  const paneOnScreen = paneShown(requested, useMediaQuery(WIDE_LAYOUT));
  const listHref = messagesHref(slug, {
    ...search,
    chat: undefined,
    thread: undefined,
  });

  function go(next: MessagesSearch) {
    void router.navigate({ href: messagesHref(slug, next) });
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col items-stretch overflow-hidden lg:flex-row">
      <div
        className={cn(
          "min-h-0 flex-1 flex-col lg:flex lg:w-thread-list lg:flex-none lg:bg-surface",
          requested === undefined ? "flex" : "hidden",
        )}
      >
        <h1 className="sr-only">Messages</h1>
        <div className="flex shrink-0 flex-col gap-10 px-16 pt-12 pb-6 lg:pt-16 lg:pb-8">
          <div className="flex items-center gap-8">
            <label className="flex min-w-0 flex-1 items-center gap-8 rounded-s bg-fill px-12 py-9 has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-2 has-[input:focus-visible]:outline-accent max-sm:min-h-44">
              <Search
                aria-hidden="true"
                size={16}
                className="shrink-0 text-text-2"
              />
              <input
                type="search"
                aria-label="Search chats"
                placeholder="Search chats, agents, tasks"
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                }}
                className="min-w-0 flex-1 bg-transparent text-15 leading-tight text-text outline-none placeholder:text-text-2"
              />
            </label>
            <Tooltip content="Send feedback">
              <IconButton
                aria-label="Send feedback"
                variant="quiet"
                className="max-sm:size-44"
                onClick={() => {
                  setFeedback(true);
                }}
              >
                <MessageSquarePlus aria-hidden="true" size={16} />
              </IconButton>
            </Tooltip>
          </div>
          <div className="flex items-center justify-between gap-8">
            <ThreadFilterMenu
              filter={filter}
              shows={showOptions(you, openAsks.size)}
              options={filterOptions(data)}
              onShow={(next) => {
                go({
                  ...withShow(search, next),
                  chat: undefined,
                  thread: undefined,
                });
              }}
              onNarrow={(key, value) => {
                go({
                  ...search,
                  chat: undefined,
                  thread: undefined,
                  [key]: value,
                });
              }}
            />
            <p className="shrink-0 text-13 text-text-2">
              {chatCount(shown.length)}
            </p>
          </div>
        </div>
        <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-8 pt-4 pb-16">
          {shown.length === 0 ? (
            <div className="px-8 py-8">
              <EmptyPanel
                icon={MessagesSquare}
                className="lg:bg-fill"
                title={
                  data.length === 0 ? "No messages yet" : "No chat matches"
                }
              >
                {data.length === 0
                  ? "Feedback you send and messages from agents appear here."
                  : "Change or clear the search and the filters to see more chats."}
              </EmptyPanel>
            </div>
          ) : (
            <ChatList
              chats={shown}
              selected={paneOnScreen ? (open?.key ?? null) : null}
              now={dataUpdatedAt}
              you={you}
              open={openAsks}
              hrefOf={(chat) =>
                messagesHref(
                  slug,
                  chat.agent === null
                    ? { ...search, chat: undefined, thread: chat.key }
                    : { ...search, chat: chat.key, thread: undefined },
                )
              }
            />
          )}
          {listing.more ? (
            <p className="px-12 pt-12 text-13 text-text-2">
              {olderNotListed(data.length, "threads")}
            </p>
          ) : null}
        </div>
      </div>
      <div
        className={cn(
          "min-h-0 min-w-0 flex-1 flex-col lg:flex",
          requested === undefined ? "hidden" : "flex",
        )}
      >
        {target.kind === "listed" ? null : (
          <div className="flex min-h-0 flex-1 flex-col gap-8 p-16 lg:p-28">
            <HrefLink
              href={listHref}
              className={cn(
                buttonClasses("quiet", "md"),
                focusRing,
                "self-start lg:hidden",
              )}
            >
              <ArrowLeft aria-hidden="true" size={15} />
              Chats
            </HrefLink>
            {target.kind === "unlisted" ? (
              <UnlistedThread
                id={target.id}
                error={unlisted.error}
                onRetry={() => void unlisted.refetch()}
              />
            ) : (
              <EmptyPanel icon={SearchX} title={missingChatTitle(target.key)}>
                No message of the project is between you and this agent.
              </EmptyPanel>
            )}
          </div>
        )}
        {open ? (
          <ChatPanel
            key={`${open.key} ${search.thread ?? ""}`}
            slug={slug}
            chat={open}
            link={search.thread}
            now={dataUpdatedAt}
            backHref={listHref}
            shown={paneOnScreen}
            open={openAsks}
            onReply={async (replyTo, body) => {
              await send.mutateAsync({ reply_to: replyTo, body });
            }}
            onAnswer={async (ask, answers) => {
              await answer.mutateAsync({ ask, answers });
            }}
          />
        ) : null}
      </div>
      <FeedbackDialog
        slug={slug}
        open={feedback}
        onOpenChange={setFeedback}
        onSend={async (task, body) => {
          const sent = await send.mutateAsync({ task, body });
          // A set filter could hide the new thread, so the screen opens it with none.
          go({ thread: sent.thread });
        }}
      />
    </div>
  );
}

function UnlistedThread({
  id,
  error,
  onRetry,
}: {
  id: string;
  error: unknown;
  onRetry: () => void;
}) {
  if (isMissingThread(error)) {
    return (
      <EmptyPanel icon={SearchX} title={`${id} does not exist`}>
        The project has no such message.
      </EmptyPanel>
    );
  }
  if (error !== null) {
    return (
      <ErrorBanner
        title="The thread could not load."
        message="The daemon did not answer."
        onRetry={onRetry}
      />
    );
  }
  return <Skeleton className="flex-1" />;
}

export function MessagesPending() {
  return (
    <div className="flex flex-col gap-16 p-16 lg:flex-row">
      <Skeleton className="lg:w-thread-list lg:shrink-0" />
      <Skeleton className="flex-1" />
    </div>
  );
}
