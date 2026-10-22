import {
  useMutation,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import { GitPullRequest, Pencil } from "lucide-react";
import { useState } from "react";
import { keys } from "@/api/keys";
import {
  addComment,
  askAuthor,
  integrateProposal,
  sendForReviewAgain,
  submitReview,
  type ChangeRange,
  type NewComment,
  type NewQuestion,
  type NewReview,
  type ProposalDetail,
  type ReviewComment,
} from "@/api/proposals";
import {
  proposalChangesQuery,
  proposalCommentsQuery,
  proposalFileDiffQuery,
  proposalQuery,
  proposalThreadsQuery,
} from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { buttonClasses } from "@/components/button/buttonStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import { DiffViewer, type FileDiffView } from "@/features/diff/DiffViewer";
import { currentFile, fileBody, listedFiles } from "@/features/diff/layout";
import type { DiffSelection } from "@/features/diff/selection";
import { revisionsQuery } from "@/api/repositoryQueries";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import {
  actedVersion,
  APPROVE_BODY,
  approveBlock,
  humanVerdict,
  integrateBlock,
  visibleActions,
  type HumanAction,
} from "./actions";
import { AskAuthorBox } from "./AskAuthorBox";
import { canEdit, editableFile } from "./edit";
import { describeFailure, reloadsProposal } from "./errors";
import { FileEditor } from "./FileEditor";
import { QUEUED_BLOCK, queuedNote } from "./checkRuns";
import { integrationOf, mainHead } from "./integration";
import {
  lineMenu,
  lineQuestions,
  questionHref,
  questionLabel,
  questionMarks,
  type LineQuestion,
} from "./lineQuestions";
import { ProposalHeader } from "./ProposalHeader";
import { QuestionMarker } from "./QuestionMarker";
import { SelectionMenu } from "./SelectionMenu";
import { SidePanel } from "./SidePanel";
import { CommentComposer, ThreadBox } from "./Threads";
import { buildThreads, heldLines, placeThreads } from "./threadsModel";
import {
  emptyChangeText,
  resolveView,
  shownVersion,
  type DetailSearch,
  type DetailView,
} from "./view";
import { projectPath } from "@/routes/paths";

interface ProposalDetailScreenProps {
  slug: string;
  id: string;
  search: DetailSearch;
  /** The signed-in human; null before the name prompt is answered. */
  human: string | null;
  onSearch: (next: DetailSearch) => void;
}

function failureText(error: unknown): string {
  return describeFailure(error).text;
}

function useFileDiff(
  slug: string,
  id: string,
  range: ChangeRange,
  path: string | null,
): FileDiffView {
  const query = useQuery({
    ...proposalFileDiffQuery(slug, id, range, path ?? ""),
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

/** A line comment or a question from the diff selection, exactly as the selection gives its range. */
type LineComment = DiffSelection & { body: string };

/** What the selected lines show: the menu, or the box of the action the human chose there. */
type SelectionMode = "menu" | "ask" | "comment";

interface DiffAreaProps {
  slug: string;
  proposal: ProposalDetail;
  comments: readonly ReviewComment[];
  questions: readonly LineQuestion[];
  view: DetailView;
  search: DetailSearch;
  human: string | null;
  now: number;
  onSearch: (next: DetailSearch) => void;
  onComment: (comment: LineComment) => Promise<void>;
  /** Resolves once the question is stored and the proposal's threads are read again. */
  onAsk: (question: LineComment) => Promise<void>;
  /** Resolves once the proposal with the human's new version is read again. */
  onEdited: (path: string) => Promise<void>;
}

function DiffArea({
  slug,
  proposal,
  comments,
  questions,
  view,
  search,
  human,
  now,
  onSearch,
  onComment,
  onAsk,
  onEdited,
}: DiffAreaProps) {
  const id = proposal.summary.id;
  const comparison = useQuery(proposalChangesQuery(slug, id, view.range));
  const [selection, setSelection] = useState<DiffSelection | null>(null);
  const [mode, setMode] = useState<SelectionMode>("menu");
  const select = (next: DiffSelection | null) => {
    setSelection(next);
    setMode("menu");
  };
  const [editing, setEditing] = useState<string | null>(null);
  const files = comparison.data ? listedFiles(comparison.data.files) : [];
  const file = currentFile(files, search.path);
  const fileDiff = useFileDiff(
    slug,
    id,
    view.range,
    file !== null && fileBody(file) === "text" ? file.path : null,
  );
  const version = shownVersion(view);
  const threads = buildThreads(comments, version);
  const byId = new Map(threads.map((t) => [t.id, t]));
  // Only the file on screen has its hunks read; a file whose text is not shown holds no line.
  const held = new Map<string, ReadonlySet<string>>();
  if (file !== null && fileBody(file) !== "text") {
    held.set(file.path, new Set());
  } else if (file !== null && fileDiff.status === "ready") {
    held.set(file.path, heldLines(fileDiff.diff.hunks));
  }
  const { placed, unplaced } = placeThreads(
    threads,
    files.map((f) => f.path),
    view.kind === "version",
    held,
  );
  const editable = canEdit(proposal, human, view) && editableFile(file);
  const questionById = new Map(questions.map((q) => [q.id, q]));

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
  if (editable && editing === file.path) {
    return (
      <FileEditor
        slug={slug}
        id={id}
        view={view}
        path={file.path}
        onCancel={() => {
          setEditing(null);
        }}
        onSaved={async () => {
          await onEdited(file.path);
        }}
      />
    );
  }
  return (
    <div className="flex min-w-0 flex-1 flex-col gap-16">
      {editable ? (
        <div className="flex justify-end">
          <Button
            size="sm"
            icon={<Pencil aria-hidden="true" size={14} />}
            onClick={() => {
              select(null);
              setEditing(file.path);
            }}
          >
            Edit {file.path}
          </Button>
        </div>
      ) : null}
      {files.length === 0 ? (
        <EmptyPanel title="No changes">{emptyChangeText(view)}</EmptyPanel>
      ) : (
        <DiffViewer
          comparison={data}
          filesTitle="Files"
          selectedPath={file?.path}
          onSelectFile={(path) => {
            select(null);
            onSearch({ ...search, path });
          }}
          fileDiff={fileDiff}
          selection={selection}
          onSelect={select}
          threads={placed}
          marks={questionMarks(questions, view)}
          renderMark={(mark, first) => {
            const question = questionById.get(mark.id);
            return question ? (
              <QuestionMarker
                href={questionHref(slug, question)}
                label={questionLabel(question)}
                answered={question.answered}
                focusable={first}
              />
            ) : null;
          }}
          renderThread={(anchor) => {
            const thread = byId.get(anchor.id);
            return thread ? (
              <ThreadBox slug={slug} thread={thread} now={now} />
            ) : null;
          }}
          renderComposer={(sel) => {
            const menu = lineMenu(proposal, human, view, sel.side, sel.path);
            const key = `${sel.path}:${sel.side}:${String(sel.start_line)}:${String(sel.end_line)}`;
            if (mode === "ask" && menu.ask !== null) {
              return (
                <AskAuthorBox
                  slug={slug}
                  key={key}
                  selection={sel}
                  version={view.version}
                  failureText={failureText}
                  onCancel={() => {
                    setMode("menu");
                  }}
                  onSubmit={async (body) => {
                    await onAsk({ ...sel, body });
                    select(null);
                  }}
                />
              );
            }
            if (mode === "comment" && menu.comment !== null) {
              return (
                <CommentComposer
                  slug={slug}
                  key={key}
                  selection={sel}
                  human={menu.comment}
                  failureText={failureText}
                  onCancel={() => {
                    setMode("menu");
                  }}
                  onSubmit={async (body) => {
                    await onComment({ ...sel, body });
                    select(null);
                  }}
                />
              );
            }
            return (
              <SelectionMenu
                slug={slug}
                id={id}
                menu={menu}
                onAsk={() => {
                  setMode("ask");
                }}
                onComment={() => {
                  setMode("comment");
                }}
              />
            );
          }}
        />
      )}
      {unplaced.length > 0 ? (
        <section aria-label="Other comments" className="flex flex-col gap-10">
          <h2 className="text-overline leading-snug font-semibold text-primary-text uppercase">
            Other comments
          </h2>
          {unplaced.map((thread) => (
            <ThreadBox key={thread.id} slug={slug} thread={thread} now={now} />
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
  const view = resolveView(data, search);
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

  return (
    <div className="flex flex-col">
      <ProposalHeader
        slug={slug}
        proposal={data}
        view={view}
        main={main}
        revision={integration?.revision ?? null}
      />
      {shown?.place === "banner" ? (
        <ErrorBanner title="The request failed." message={shown.text} />
      ) : null}
      <div className="flex items-start gap-20 px-24 pt-16 pb-24">
        <DiffArea
          key={`${String(view.range.version)}:${String(view.range.since)}`}
          slug={slug}
          proposal={data}
          comments={comments.data ?? []}
          questions={lineQuestions(threads.data?.items ?? [], id)}
          view={view}
          search={search}
          human={human}
          now={now}
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
            onSearch({ path });
          }}
        />
        <SidePanel
          slug={slug}
          proposal={data}
          view={view}
          human={human}
          now={now}
          main={main}
          integration={integration}
          queued={queued}
          actions={{
            visible: visibleActions(data, human),
            pending,
            failure: shown?.place === "inline" ? shown.text : null,
            integrateBlock:
              queued === null ? integrateBlock(data, main) : QUEUED_BLOCK,
            approveBlock: approveBlock(data),
            onApprove: () => {
              review.mutate(humanVerdict(view, "approve", APPROVE_BODY));
            },
            onIntegrate: () => {
              integrate.mutate(actedVersion(view));
            },
            onReviewAgain: () => {
              reviewAgain.mutate(actedVersion(view));
            },
            onRequestChanges: async (body) => {
              await review.mutateAsync(
                humanVerdict(view, "request_changes", body),
              );
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
