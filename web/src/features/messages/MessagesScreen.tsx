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
  GitPullRequest,
  MessageSquarePlus,
  MessagesSquare,
  SearchX,
  SquareKanban,
} from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import { sendMessage, type NewMessage } from "@/api/messages";
import { olderNotListed } from "@/api/pages";
import { messagesQuery, threadQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { buttonClasses } from "@/components/button/buttonStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { Button } from "@/components/button/Button";
import { Menu } from "@/components/overlay/Menu";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import {
  filterLabel,
  filterOptions,
  filterThreads,
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

export function MessagesScreen({ slug, search }: MessagesScreenProps) {
  const { data: listing, dataUpdatedAt } = useSuspenseQuery(
    messagesQuery(slug),
  );
  const data = listing.items;
  const router = useRouter();
  const queryClient = useQueryClient();
  const [feedback, setFeedback] = useState(false);
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
  };
  const options = filterOptions(data);
  const visible = filterThreads(data, filter);
  const target = openThread(data, visible, search.thread);
  // A thread the listing did not reach is read on its own.
  const unlisted = useQuery({
    ...threadQuery(slug, target.kind === "unlisted" ? target.id : ""),
    enabled: target.kind === "unlisted",
  });
  const open =
    target.kind === "listed" ? target.thread : (unlisted.data ?? null);

  function go(next: MessagesSearch) {
    void router.navigate({ href: messagesHref(slug, next) });
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col items-stretch gap-20 overflow-hidden px-24 pt-20 pb-24 lg:flex-row">
      <div
        className={cn(
          "min-h-0 flex-1 flex-col gap-12 lg:flex lg:w-thread-list lg:flex-none",
          search.thread === undefined ? "flex" : "hidden",
        )}
      >
        <div className="flex shrink-0 flex-col gap-4">
          <h1 className="text-screen leading-snug font-semibold text-text">
            Messages
          </h1>
          <p className="text-body text-text-muted">
            Feedback from humans and messages from agents without native
            messaging. Claude Code sessions talk to each other in their own
            terminals.
          </p>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-6">
          {FILTERS.map(({ key, label, all, icon: Icon, options: group }) => {
            const value = search[key];
            const pick = (next: string | undefined) => {
              go({ ...search, thread: undefined, [key]: next });
            };
            return (
              <Menu
                key={key}
                trigger={
                  <Button icon={<Icon aria-hidden="true" size={15} />}>
                    {filterLabel(label, value)}
                  </Button>
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
          <Button
            icon={<MessageSquarePlus aria-hidden="true" size={14} />}
            onClick={() => {
              setFeedback(true);
            }}
          >
            Send feedback
          </Button>
        </div>
        <div className="flex min-h-0 flex-1 flex-col gap-12 overflow-y-auto">
          {visible.length === 0 ? (
            <EmptyPanel
              icon={MessagesSquare}
              title={
                data.length === 0 ? "No messages yet" : "No thread matches"
              }
            >
              {data.length === 0
                ? "Feedback you send and messages from agents appear here."
                : "Change or clear the filters to see more threads."}
            </EmptyPanel>
          ) : (
            <ThreadList
              threads={visible}
              selected={open?.id ?? null}
              now={dataUpdatedAt}
              hrefOf={(thread) =>
                messagesHref(slug, { ...search, thread: thread.id })
              }
            />
          )}
          {listing.more ? (
            <p className="text-bar text-text-muted">
              {olderNotListed(data.length, "threads")}
            </p>
          ) : null}
        </div>
      </div>
      <div
        className={cn(
          "min-h-0 min-w-0 flex-1 flex-col gap-8 lg:flex",
          search.thread === undefined ? "hidden" : "flex",
        )}
      >
        <HrefLink
          href={messagesHref(slug, { ...search, thread: undefined })}
          className={cn(
            buttonClasses("secondary", "md"),
            focusRing,
            "self-start lg:hidden",
          )}
        >
          <ArrowLeft aria-hidden="true" size={15} />
          Threads
        </HrefLink>
        {target.kind === "unlisted" && !unlisted.data ? (
          <UnlistedThread
            id={target.id}
            error={unlisted.error}
            onRetry={() => void unlisted.refetch()}
          />
        ) : null}
        {open ? (
          <ThreadPanel
            key={open.id}
            slug={slug}
            thread={open}
            now={dataUpdatedAt}
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
    <div className="flex flex-col gap-20 px-24 pt-20 pb-24 lg:flex-row">
      <Skeleton className="lg:w-thread-list lg:shrink-0" />
      <Skeleton className="flex-1" />
    </div>
  );
}
