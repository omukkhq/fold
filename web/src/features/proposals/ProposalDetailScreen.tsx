import {
  useMutation,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import { GitPullRequest } from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import {
  addComment,
  integrateProposal,
  submitReview,
  type NewComment,
  type NewReview,
  type ProposalDetail,
} from "@/api/proposals";
import { comparisonQuery, fileDiffQuery, proposalQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { buttonClasses } from "@/components/button/buttonStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { DiffViewer, type FileDiffView } from "@/features/diff/DiffViewer";
import { currentFile, fileBody, listedFiles } from "@/features/diff/layout";
import type { DiffSelection } from "@/features/diff/selection";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import {
  actionErrorText,
  canComment,
  visibleActions,
  type HumanAction,
} from "./actions";
import { ProposalHeader } from "./ProposalHeader";
import { SidePanel } from "./SidePanel";
import { CommentComposer, ThreadBox } from "./Threads";
import { buildThreads, diffThreads, unplacedThreads } from "./threads";
import {
  resolveView,
  shownVersion,
  type DetailSearch,
  type DetailView,
} from "./view";

interface ProposalDetailScreenProps {
  slug: string;
  id: string;
  search: DetailSearch;
  /** The signed-in human; null before the name prompt is answered. */
  human: string | null;
  onSearch: (next: DetailSearch) => void;
}

function failureMessage(error: unknown): string {
  return (
    actionErrorText(error) ??
    (error instanceof Error && error.message
      ? error.message
      : "The request failed.")
  );
}

function useFileDiff(
  slug: string,
  from: string | undefined,
  to: string,
  path: string | null,
): FileDiffView {
  const query = useQuery({
    ...fileDiffQuery(slug, from, to, path ?? ""),
    enabled: path !== null,
  });
  if (query.isError) {
    return {
      status: "error",
      retry: () => {
        void query.refetch();
      },
    };
  }
  return query.data
    ? { status: "ready", diff: query.data }
    : { status: "pending" };
}

interface DiffAreaProps {
  slug: string;
  proposal: ProposalDetail;
  view: DetailView;
  search: DetailSearch;
  human: string | null;
  now: number;
  onSearch: (next: DetailSearch) => void;
  onComment: (comment: NewComment) => Promise<void>;
}

function DiffArea({
  slug,
  proposal,
  view,
  search,
  human,
  now,
  onSearch,
  onComment,
}: DiffAreaProps) {
  const from = view.kind === "compare" ? view.fromRef : undefined;
  const comparison = useQuery(comparisonQuery(slug, from, view.to));
  const [selection, setSelection] = useState<DiffSelection | null>(null);
  const files = comparison.data ? listedFiles(comparison.data.files) : [];
  const file = currentFile(files, search.path);
  const fileDiff = useFileDiff(
    slug,
    from,
    view.to,
    file !== null && fileBody(file) === "text" ? file.path : null,
  );
  const version = shownVersion(view);
  const threads =
    version === null ? [] : buildThreads(proposal.comments, version);
  const byId = new Map(threads.map((t) => [t.id, t]));
  const unplaced = unplacedThreads(
    threads,
    files.map((f) => f.path),
  );
  const composer = human !== null && canComment(proposal, human, version);

  if (comparison.isPending) return <Skeleton className="min-w-0 flex-1" />;
  if (comparison.isError) {
    return (
      <ErrorBanner
        className="min-w-0 flex-1"
        title="The diff could not load."
        message="The comparison did not arrive."
        onRetry={() => void comparison.refetch()}
      />
    );
  }
  const data = comparison.data;
  return (
    <div className="flex min-w-0 flex-1 flex-col gap-16">
      {data === null || files.length === 0 ? (
        <EmptyPanel title={data === null ? "No diff" : "No changes"}>
          {data === null
            ? `fold has no diff for ${view.kind === "compare" ? `${view.fromRef} and ${view.to}` : view.to}.`
            : `${view.to} holds the same files as its base.`}
        </EmptyPanel>
      ) : (
        <DiffViewer
          comparison={data}
          filesTitle="Files"
          selectedPath={file?.path}
          onSelectFile={(path) => {
            setSelection(null);
            onSearch({ ...search, path });
          }}
          fileDiff={fileDiff}
          selection={selection}
          onSelect={setSelection}
          threads={diffThreads(threads)}
          renderThread={(anchor) => {
            const thread = byId.get(anchor.id);
            return thread ? <ThreadBox thread={thread} now={now} /> : null;
          }}
          renderComposer={
            composer && version !== null
              ? (sel) => (
                  <CommentComposer
                    key={`${sel.path}:${sel.side}:${String(sel.start_line)}:${String(sel.end_line)}`}
                    selection={sel}
                    human={human}
                    failureText={failureMessage}
                    onCancel={() => {
                      setSelection(null);
                    }}
                    onSubmit={async (body) => {
                      await onComment({ version, ...sel, body });
                      setSelection(null);
                    }}
                  />
                )
              : undefined
          }
        />
      )}
      {unplaced.length > 0 ? (
        <section aria-label="Other comments" className="flex flex-col gap-10">
          <h2 className="text-overline leading-snug font-semibold text-primary-text uppercase">
            Other comments
          </h2>
          {unplaced.map((thread) => (
            <ThreadBox key={thread.id} thread={thread} now={now} />
          ))}
        </section>
      ) : null}
    </div>
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
  const queryClient = useQueryClient();
  const now = useNow(true, 30_000);
  const [failure, setFailure] = useState<unknown>(null);

  // The daemon answers a write with the proposal as it now stands, so the screen shows that answer; nothing changes before it.
  const settle = (detail: ProposalDetail) => {
    queryClient.setQueryData(keys.proposal(slug, id), detail);
    void queryClient.invalidateQueries({
      queryKey: keys.proposals(slug),
      exact: true,
    });
  };
  const handlers = {
    onMutate: () => {
      setFailure(null);
    },
    onSuccess: settle,
    onError: (error: unknown) => {
      setFailure(error);
    },
  };
  const review = useMutation({
    mutationFn: (input: NewReview) => submitReview(slug, id, input),
    ...handlers,
  });
  const integrate = useMutation({
    mutationFn: (version: number) => integrateProposal(slug, id, { version }),
    ...handlers,
  });
  const comment = useMutation({
    mutationFn: (input: NewComment) => addComment(slug, id, input),
    onSuccess: settle,
  });

  if (data === null) {
    return (
      <div className="p-24">
        <EmptyPanel
          icon={GitPullRequest}
          title={`No proposal ${id}`}
          action={
            <HrefLink
              href={`/p/${slug}/proposals`}
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

  const view = resolveView(data, search);
  const pending: HumanAction | null = review.isPending
    ? review.variables.verdict
    : integrate.isPending
      ? "integrate"
      : null;
  const mapped = failure === null ? null : actionErrorText(failure);

  return (
    <div className="flex flex-col">
      <ProposalHeader slug={slug} proposal={data} view={view} />
      {failure !== null && mapped === null ? (
        <ErrorBanner
          title="The request failed."
          message={failureMessage(failure)}
        />
      ) : null}
      <div className="flex items-start gap-20 px-24 pt-16 pb-24">
        <DiffArea
          key={`${view.kind}:${view.to}`}
          slug={slug}
          proposal={data}
          view={view}
          search={search}
          human={human}
          now={now}
          onSearch={onSearch}
          onComment={async (input) => {
            await comment.mutateAsync(input);
          }}
        />
        <SidePanel
          slug={slug}
          proposal={data}
          view={view}
          human={human}
          now={now}
          actions={{
            visible: visibleActions(data, human),
            pending,
            failure: mapped,
            onApprove: () => {
              review.mutate({
                version: data.current_version,
                verdict: "approve",
                body: "",
              });
            },
            onIntegrate: () => {
              integrate.mutate(data.current_version);
            },
            onRequestChanges: async (body) => {
              await review.mutateAsync({
                version: data.current_version,
                verdict: "request_changes",
                body,
              });
            },
          }}
        />
      </div>
    </div>
  );
}

export function ProposalDetailPending() {
  return (
    <div className="flex flex-col gap-16 px-24 pt-18 pb-24">
      <Skeleton />
      <div className="flex items-start gap-20">
        <Skeleton className="flex-1" />
        <Skeleton className="w-review-panel shrink-0" />
      </div>
    </div>
  );
}
