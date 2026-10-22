import * as RadixTabs from "@radix-ui/react-tabs";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import { GitPullRequest } from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import { sendMessage, type NewMessage } from "@/api/messages";
import {
  addComment,
  askAuthor,
  integrateProposal,
  sendForReviewAgain,
  submitReview,
  type CheckRun,
  type NewComment,
  type NewQuestion,
  type NewReview,
} from "@/api/proposals";
import {
  proposalCommentsQuery,
  proposalQuery,
  proposalThreadsQuery,
  proposalVersionQuery,
  workersQuery,
} from "@/api/queries";
import { olderNotListed } from "@/api/pages";
import { revisionsQuery } from "@/api/repositoryQueries";
import { HrefLink } from "@/app/HrefLink";
import { buttonClasses } from "@/components/button/buttonStyles";
import { TAB_BY_STATE, TAB_LIST } from "@/components/overlay/tabStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import { projectPath } from "@/routes/paths";
import {
  actedVersion,
  APPROVE_BODY,
  approveBlock,
  humanVerdict,
  integrateBlock,
  visibleActions,
  type HumanAction,
} from "./actions";
import { ChangesTab } from "./ChangesTab";
import { QUEUED_BLOCK, queuedNote } from "./checkRuns";
import { ChecksTab } from "./ChecksTab";
import { allRuns } from "./runList";
import { conversationThreads } from "./conversation";
import { ConversationTab } from "./ConversationTab";
import { describeFailure, reloadsProposal } from "./errors";
import type { ActionHandlers } from "./HumanActions";
import { integrationOf, mainHead } from "./integration";
import { lineQuestions } from "./lineQuestions";
import { OverviewTab } from "./OverviewTab";
import { authorArea } from "./present";
import { ProposalHeader } from "./ProposalHeader";
import { runsCount } from "./stateLine";
import {
  detailHref,
  detailTab,
  isDetailTab,
  resolveView,
  tabSearch,
  type DetailSearch,
  type DetailTab,
  type DetailView,
} from "./view";

interface ProposalDetailScreenProps {
  slug: string;
  id: string;
  search: DetailSearch;
  /** The signed-in human; null before the name prompt is answered. */
  human: string | null;
  /** A tab change pushes a history entry, so Back returns to the earlier tab; a file, version or run on the same tab replaces the entry. */
  onSearch: (next: DetailSearch, history?: "push" | "replace") => void;
}

function TabTrigger({
  value,
  label,
  count,
}: {
  value: DetailTab;
  label: string;
  count: string | null;
}) {
  return (
    <RadixTabs.Trigger value={value} className={cn(TAB_BY_STATE, focusRing)}>
      {label}
      {count === null ? null : (
        <span className="rounded-s bg-sunken px-6 text-caption font-medium text-text-muted">
          {count}
        </span>
      )}
    </RadixTabs.Trigger>
  );
}

export function ProposalDetailScreen({
  slug,
  id,
  search,
  human,
  onSearch,
}: ProposalDetailScreenProps) {
  const { data } = useSuspenseQuery(proposalQuery(slug, id));
  const comments = useQuery({
    ...proposalCommentsQuery(slug, id),
    enabled: data !== null,
  });
  const revisions = useQuery({
    ...revisionsQuery(slug),
    enabled: data !== null,
  });
  const threads = useQuery({
    ...proposalThreadsQuery(slug, id),
    enabled: data !== null,
  });
  const workers = useQuery({ ...workersQuery(slug), enabled: data !== null });
  const current = data?.summary.current_version ?? 0;
  // The proposal read carries the check runs of one version only, so each earlier version is read for its runs.
  const earlier = (data?.versions ?? [])
    .map((v) => v.version)
    .filter((v) => v !== current);
  const earlierReads = useQueries({
    queries: earlier.map((version) => proposalVersionQuery(slug, id, version)),
  });
  const queryClient = useQueryClient();
  const now = useNow(true, 30_000);
  const [failure, setFailure] = useState<unknown>(null);

  // The write answers before the proposal is read again; the action stays pending until the new proposal arrives, so the screen never shows a result the daemon has not sent.
  const reload = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: keys.proposal(slug, id) }),
      queryClient.invalidateQueries({
        queryKey: keys.proposals(slug),
        exact: true,
      }),
    ]);
  };
  const handlers = {
    onMutate: () => {
      setFailure(null);
    },
    onError: async (error: unknown) => {
      setFailure(error);
      if (reloadsProposal(error)) await reload();
    },
  };
  const review = useMutation({
    mutationFn: (input: NewReview) => submitReview(slug, id, input),
    onSuccess: reload,
    ...handlers,
  });
  const integrate = useMutation({
    mutationFn: (version: number) => integrateProposal(slug, id, version),
    onSuccess: async () => {
      await Promise.all([
        reload(),
        queryClient.invalidateQueries({ queryKey: keys.repository(slug) }),
      ]);
    },
    ...handlers,
  });
  const comment = useMutation({
    mutationFn: (input: NewComment) => addComment(slug, id, input),
    onSuccess: reload,
  });
  const ask = useMutation({
    mutationFn: (input: NewQuestion) => askAuthor(slug, id, input),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: keys.proposalThreads(slug, id),
      });
    },
  });
  const reply = useMutation({
    mutationFn: (input: NewMessage) => sendMessage(slug, input),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: keys.messages(slug) });
    },
  });
  const reviewAgain = useMutation({
    mutationFn: (version: number) => sendForReviewAgain(slug, id, version),
    onSuccess: reload,
    ...handlers,
  });

  if (data === null) {
    return (
      <div className="p-24">
        <EmptyPanel
          icon={GitPullRequest}
          title={`No proposal ${id}`}
          action={
            <HrefLink
              href={projectPath(slug, "proposals")}
              className={cn(buttonClasses("secondary", "md"), focusRing)}
            >
              Open the proposals
            </HrefLink>
          }
        >
          fold has no proposal with this id in this project.
        </EmptyPanel>
      </div>
    );
  }

  const summary = data.summary;
  const tab = detailTab(search);
  const view = resolveView(data, search);
  const currentView = resolveView(data, {});
  const revisionList = revisions.data?.items ?? [];
  const main = mainHead(revisionList);
  const integration = integrationOf(summary, revisionList, integrate.data);
  const queued = queuedNote(summary);
  const pending: HumanAction | null = review.isPending
    ? review.variables.verdict === "approve"
      ? "approve"
      : "request_changes"
    : integrate.isPending
      ? "integrate"
      : reviewAgain.isPending
        ? "review_again"
        : null;
  const shown = failure === null ? null : describeFailure(failure);

  const runs = new Map<number, readonly CheckRun[]>([[current, data.checks]]);
  const failedVersions = new Set<number>();
  earlier.forEach((version, index) => {
    const read = earlierReads[index];
    if (read?.data) runs.set(version, read.data.checks);
    else if (read?.isError) failedVersions.add(version);
  });
  // A version whose read failed leaves its runs out rather than holding the Checks tab in its loading state.
  const settled = earlierReads.every((read) => !read.isPending);
  const runItems = settled ? allRuns(runs) : null;
  const conversation =
    threads.data && comments.data
      ? conversationThreads(data, threads.data.items, comments.data)
      : null;

  const actionsFor = (acted: DetailView): ActionHandlers => ({
    visible: visibleActions(data, human),
    pending,
    failure: shown?.place === "inline" ? shown.text : null,
    integrateBlock: queued === null ? integrateBlock(data, main) : QUEUED_BLOCK,
    approveBlock: approveBlock(data),
    onApprove: () => {
      review.mutate(humanVerdict(acted, "approve", APPROVE_BODY));
    },
    onIntegrate: () => {
      integrate.mutate(actedVersion(acted));
    },
    onReviewAgain: () => {
      reviewAgain.mutate(actedVersion(acted));
    },
    onRequestChanges: async (body) => {
      await review.mutateAsync(humanVerdict(acted, "request_changes", body));
    },
  });
  const tabHref = (next: DetailSearch) => detailHref(slug, id, next);
  // A panel spans the page, so its ring is drawn inside it where no edge clips it.
  const content = insetFocusRing;

  return (
    <div className="flex min-w-0 flex-col">
      <ProposalHeader
        slug={slug}
        proposal={data}
        main={main}
        revision={integration?.revision ?? null}
        batch={integration?.batch ?? null}
        now={now}
      />
      {shown?.place === "banner" ? (
        <ErrorBanner
          className="mx-16 mt-12 md:mx-24"
          title="The request failed."
          message={shown.text}
        />
      ) : null}
      <RadixTabs.Root
        value={tab}
        onValueChange={(value) => {
          if (isDetailTab(value)) onSearch(tabSearch(search, value), "push");
        }}
        className="flex min-w-0 flex-col"
      >
        <div className="px-16 pt-8 md:px-24">
          <RadixTabs.List
            aria-label="Proposal"
            className={cn(TAB_LIST, "flex-wrap")}
          >
            <TabTrigger value="overview" label="Overview" count={null} />
            <TabTrigger
              value="changes"
              label="Changes"
              count={String(data.totals.files)}
            />
            <TabTrigger
              value="checks"
              label="Checks"
              count={runsCount(runItems?.length ?? null)}
            />
            <TabTrigger
              value="conversation"
              label="Conversation"
              count={conversation === null ? null : String(conversation.length)}
            />
          </RadixTabs.List>
        </div>
        <RadixTabs.Content value="overview" className={content}>
          <OverviewTab
            slug={slug}
            proposal={data}
            human={human}
            now={now}
            main={main}
            integration={integration}
            queued={queued}
            actions={actionsFor(currentView)}
            actionView={currentView}
            runs={runs}
            failedVersions={failedVersions}
            comments={comments.data ?? []}
            threads={conversation}
            area={authorArea(workers.data, summary.author)}
            tabHref={tabHref}
          />
        </RadixTabs.Content>
        <RadixTabs.Content value="changes" className={content}>
          <ChangesTab
            slug={slug}
            proposal={data}
            comments={comments.data ?? []}
            questions={lineQuestions(threads.data?.items ?? [], id)}
            view={view}
            search={search}
            human={human}
            now={now}
            runs={runs.get(view.version) ?? null}
            actions={actionsFor(view)}
            onSearch={onSearch}
            onComment={async ({ path, side, start_line, end_line, body }) => {
              await comment.mutateAsync({
                version: view.version,
                path,
                side,
                start_line,
                end_line,
                body,
                reply_to: null,
              });
            }}
            onAsk={async ({ path, side, start_line, end_line, body }) => {
              await ask.mutateAsync({
                version: view.version,
                path,
                side,
                start_line,
                end_line,
                body,
              });
            }}
            onEdited={async (path) => {
              await reload();
              onSearch({ tab: "changes", path });
            }}
          />
        </RadixTabs.Content>
        <RadixTabs.Content value="checks" className={content}>
          <ChecksTab
            slug={slug}
            id={id}
            items={runItems}
            search={search}
            now={now}
          />
        </RadixTabs.Content>
        <RadixTabs.Content value="conversation" className={content}>
          <ConversationTab
            slug={slug}
            proposal={data}
            threads={conversation}
            olderNote={
              threads.data?.more
                ? olderNotListed(threads.data.items.length, "message threads")
                : null
            }
            human={human}
            now={now}
            changesHref={tabHref({ tab: "changes" })}
            onAsk={async (body) => {
              await ask.mutateAsync({ version: current, body });
            }}
            onReply={async (replyTo, body) => {
              await reply.mutateAsync({ reply_to: replyTo, body });
            }}
            onComment={async (replyTo, body) => {
              await comment.mutateAsync({
                version: current,
                body,
                reply_to: replyTo,
              });
            }}
          />
        </RadixTabs.Content>
      </RadixTabs.Root>
    </div>
  );
}

export function ProposalDetailPending() {
  return (
    <div className="flex flex-col gap-16 px-16 pt-18 pb-24 md:px-24">
      <Skeleton />
      <Skeleton className="flex-1" />
    </div>
  );
}
