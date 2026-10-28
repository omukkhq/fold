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
import { sendMessage, type NewMessage } from "@/api/messages";
import { olderNotListed } from "@/api/pages";
import { messagesQuery, sessionQuery, threadQuery } from "@/api/queries";
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
import {
  filterOptions,
  filterThreads,
  searchThreads,
  showOptions,
  threadCount,
  withShow,
  type ThreadFilter,
} from "./filters";
import { FeedbackDialog } from "./FeedbackDialog";
import { isMissingThread, openThread } from "./openThread";
import { messagesHref, type MessagesSearch } from "./search";
import { ThreadFilterMenu } from "./ThreadFilterMenu";
import { ThreadList } from "./ThreadList";
import { ThreadPanel } from "./ThreadPanel";
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

  const filter: ThreadFilter = {
    agent: search.agent,
    task: search.task,
    proposal: search.proposal,
    waiting: search.waiting,
    questions: search.questions,
    mine: search.mine,
  };
  const visible = filterThreads(data, filter, you);
  const shown = searchThreads(visible, query);
  const target = openThread(data, visible, search.thread);
  // A thread the listing did not reach is read on its own.
  const unlisted = useQuery({
    ...threadQuery(slug, target.kind === "unlisted" ? target.id : ""),
    enabled: target.kind === "unlisted",
  });
  const open =
    target.kind === "listed" ? target.thread : (unlisted.data ?? null);
  const paneOnScreen = paneShown(search.thread, useMediaQuery(WIDE_LAYOUT));
  const listHref = messagesHref(slug, { ...search, thread: undefined });

  function go(next: MessagesSearch) {
    void router.navigate({ href: messagesHref(slug, next) });
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col items-stretch overflow-hidden lg:flex-row">
      <div
        className={cn(
          "min-h-0 flex-1 flex-col lg:flex lg:w-thread-list lg:flex-none lg:bg-surface",
          search.thread === undefined ? "flex" : "hidden",
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
                aria-label="Search threads"
                placeholder="Search threads, agents, tasks"
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
              shows={showOptions(data, you)}
              options={filterOptions(data)}
              onShow={(next) => {
                go({ ...withShow(search, next), thread: undefined });
              }}
              onNarrow={(key, value) => {
                go({ ...search, thread: undefined, [key]: value });
              }}
            />
            <p className="shrink-0 text-13 text-text-2">
              {threadCount(shown.length)}
            </p>
          </div>
        </div>
        <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-8 pt-4 pb-16">
          {shown.length === 0 ? (
            <div className="px-8 py-8">
              <EmptyPanel
                icon={MessagesSquare}
                title={
                  data.length === 0 ? "No messages yet" : "No thread matches"
                }
              >
                {data.length === 0
                  ? "Feedback you send and messages from agents appear here."
                  : "Change or clear the search and the filters to see more threads."}
              </EmptyPanel>
            </div>
          ) : (
            <ThreadList
              threads={shown}
              selected={paneOnScreen ? (open?.id ?? null) : null}
              now={dataUpdatedAt}
              you={you}
              hrefOf={(thread) =>
                messagesHref(slug, { ...search, thread: thread.id })
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
          search.thread === undefined ? "hidden" : "flex",
        )}
      >
        {target.kind === "unlisted" && !unlisted.data ? (
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
              Threads
            </HrefLink>
            <UnlistedThread
              id={target.id}
              error={unlisted.error}
              onRetry={() => void unlisted.refetch()}
            />
          </div>
        ) : null}
        {open ? (
          <ThreadPanel
            key={open.id}
            slug={slug}
            thread={open}
            now={dataUpdatedAt}
            backHref={listHref}
            shown={paneOnScreen}
            onReply={async (replyTo, body) => {
              await send.mutateAsync({ reply_to: replyTo, body });
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
