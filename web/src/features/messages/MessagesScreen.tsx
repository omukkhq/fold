import {
  useMutation,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import {
  ArrowLeft,
  Bot,
  Check,
  ChevronDown,
  GitPullRequest,
  MessageSquarePlus,
  MessagesSquare,
  Search,
  SearchX,
  SquareKanban,
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
import { IconButton } from "@/components/button/Button";
import { Tooltip } from "@/components/overlay/Tooltip";
import { Menu } from "@/components/overlay/Menu";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import {
  filterLabel,
  filterOptions,
  filterThreads,
  searchThreads,
  showChips,
  showFilter,
  withShow,
  type ThreadFilter,
} from "./filters";
import { FeedbackDialog } from "./FeedbackDialog";
import { isMissingThread, openThread } from "./openThread";
import { messagesHref, type MessagesSearch } from "./search";
import { ThreadList } from "./ThreadList";
import { ThreadPanel } from "./ThreadPanel";

interface MessagesScreenProps {
  slug: string;
  search: MessagesSearch;
}

const FILTERS = [
  {
    key: "agent",
    label: "Agent",
    all: "All agents",
    icon: Bot,
    options: "agents",
  },
  {
    key: "task",
    label: "Task",
    all: "All tasks",
    icon: SquareKanban,
    options: "tasks",
  },
  {
    key: "proposal",
    label: "Proposal",
    all: "All proposals",
    icon: GitPullRequest,
    options: "proposals",
  },
] as const;

const CHIP =
  "inline-flex items-center gap-6 rounded-chip bg-surface px-10 py-3 text-code whitespace-nowrap inset-ring inset-ring-border max-sm:min-h-44";

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
  const options = filterOptions(data);
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
  const show = showFilter(filter);
  const listHref = messagesHref(slug, { ...search, thread: undefined });

  function go(next: MessagesSearch) {
    void router.navigate({ href: messagesHref(slug, next) });
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col items-stretch overflow-hidden lg:flex-row">
      <div
        className={cn(
          "min-h-0 flex-1 flex-col lg:flex lg:w-thread-list lg:flex-none lg:border-r lg:border-border-subtle",
          search.thread === undefined ? "flex" : "hidden",
        )}
      >
        <h1 className="sr-only">Messages</h1>
        <div className="flex shrink-0 flex-col gap-12 border-b border-border-subtle px-16 pt-16 pb-12">
          <div className="flex items-center gap-8">
            <label className="flex min-w-0 flex-1 items-center gap-8 rounded-m bg-surface px-10 py-7 inset-ring inset-ring-border has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-2 has-[input:focus-visible]:outline-focus-ring max-sm:min-h-44">
              <Search
                aria-hidden="true"
                size={15}
                className="shrink-0 text-text-faint"
              />
              <input
                type="search"
                aria-label="Search threads"
                placeholder="Search threads, agents, tasks"
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                }}
                className="min-w-0 flex-1 bg-transparent text-body leading-snug text-text outline-none placeholder:text-text-faint"
              />
            </label>
            <Tooltip content="Send feedback">
              <IconButton
                aria-label="Send feedback"
                variant="secondary"
                className="max-sm:size-44"
                onClick={() => {
                  setFeedback(true);
                }}
              >
                <MessageSquarePlus aria-hidden="true" size={15} />
              </IconButton>
            </Tooltip>
          </div>
          <div
            role="group"
            aria-label="Show"
            className="flex flex-wrap items-center gap-6"
          >
            {showChips(data, you).map((chip) => (
              <button
                key={chip.show}
                type="button"
                aria-pressed={show === chip.show}
                onClick={() => {
                  go({ ...withShow(search, chip.show), thread: undefined });
                }}
                className={cn(
                  CHIP,
                  show === chip.show
                    ? "bg-ink font-semibold text-on-ink inset-ring-ink"
                    : chip.show === "waiting"
                      ? "text-warning hover:bg-sunken"
                      : "text-text hover:bg-sunken",
                  focusRing,
                )}
              >
                {chip.label}
              </button>
            ))}
          </div>
          <div
            role="group"
            aria-label="Filters"
            className="flex flex-wrap items-center gap-6"
          >
            {FILTERS.map(({ key, label, all, icon: Icon, options: group }) => {
              const value = search[key];
              const pick = (next: string | undefined) => {
                go({ ...search, thread: undefined, [key]: next });
              };
              return (
                <Menu
                  key={key}
                  trigger={
                    <button
                      type="button"
                      className={cn(
                        CHIP,
                        value === undefined
                          ? "text-text-muted hover:bg-sunken"
                          : "bg-primary-soft text-primary-text inset-ring-primary",
                        focusRing,
                      )}
                    >
                      <Icon aria-hidden="true" size={13} />
                      {filterLabel(label, value)}
                      <ChevronDown aria-hidden="true" size={13} />
                    </button>
                  }
                  items={[
                    {
                      label: all,
                      icon: value === undefined ? Check : undefined,
                      onSelect: () => {
                        pick(undefined);
                      },
                    },
                    ...options[group].map((option) => ({
                      label: option,
                      icon: option === value ? Check : undefined,
                      onSelect: () => {
                        pick(option);
                      },
                    })),
                  ]}
                />
              );
            })}
          </div>
        </div>
        <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
          {shown.length === 0 ? (
            <div className="p-16">
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
              selected={open?.id ?? null}
              now={dataUpdatedAt}
              you={you}
              hrefOf={(thread) =>
                messagesHref(slug, { ...search, thread: thread.id })
              }
            />
          )}
          {listing.more ? (
            <p className="px-16 py-12 text-bar text-text-muted">
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
                buttonClasses("secondary", "md"),
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
