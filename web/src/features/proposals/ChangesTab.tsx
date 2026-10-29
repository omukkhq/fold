import { useQuery } from "@tanstack/react-query";
import { CheckCircle2, ChevronDown, CircleAlert, Pencil } from "lucide-react";
import { useState } from "react";
import type { CheckRun, ProposalDetail, ReviewComment } from "@/api/proposals";
import {
  proposalChangesQuery,
  proposalFileDiffQuery,
  proposalFileQuery,
  requestsQuery,
} from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { Menu } from "@/components/overlay/Menu";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { Skeleton } from "@/components/states/Skeleton";
import {
  needsFileText,
  useFileDiff,
  useFileText,
} from "@/features/diff/fileDiff";
import type { LineSet } from "@/features/diff/gaps";
import { currentFile, fileBody, listedFiles } from "@/features/diff/layout";
import type { DiffSelection } from "@/features/diff/selection";
import {
  makeRequestTitle,
  requestMarks,
  type CodeAnchor,
} from "@/features/requests/codeLocation";
import { useCodeRequest } from "@/features/requests/useCodeRequest";
import { MakeRequestBox } from "@/features/requests/MakeRequestBox";
import { RequestMarker } from "@/features/requests/RequestMarker";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ActorText } from "@/people/ActorText";
import { AskAuthorBox } from "./AskAuthorBox";
import { canEdit, editableFile } from "./edit";
import { describeFailure } from "./errors";
import { FileEditor } from "./FileEditor";
import { versionIntent } from "./intent";
import { ChangedFiles } from "./ChangedFiles";
import { useChangedFiles } from "./changedFilesState";
import {
  lineMenu,
  questionHref,
  questionLabel,
  questionMarks,
  type LineQuestion,
} from "./lineQuestions";
import { QuestionMarker } from "./QuestionMarker";
import { SelectionMenu } from "./SelectionMenu";
import { approvalLine, checksWord } from "./stateLine";
import { CommentComposer, ThreadBox } from "./Threads";
import { buildThreads, heldFileLines, placeThreads } from "./threadsModel";
import {
  changeOptions,
  detailHref,
  emptyChangeText,
  shownVersion,
  versionTabs,
  type DetailSearch,
  type DetailView,
  commentsOnSide,
} from "./view";
import { shownPath, viewedKey } from "./viewed";

function failureText(error: unknown): string {
  return describeFailure(error).text;
}

/** A line comment or a question from the diff selection, exactly as the selection gives its range. */
export type LineComment = DiffSelection & { body: string };

/** What the selected lines show: the menu, or the box of the action the human chose there. */
type SelectionMode = "menu" | "ask" | "comment" | "request";

function Toolbar({
  slug,
  proposal,
  view,
  runs,
  onSearch,
}: {
  slug: string;
  proposal: ProposalDetail;
  view: DetailView;
  /** The shown version's runs; null until they arrive. */
  runs: readonly CheckRun[] | null;
  onSearch: (next: DetailSearch) => void;
}) {
  const id = proposal.summary.id;
  const tabs = versionTabs(proposal, view);
  const shown = tabs.find((tab) => tab.selected);
  const options = changeOptions(proposal, view);
  const approval = approvalLine(proposal, view.version);
  const checks = checksWord(runs);
  const StateIcon = approval.tone === "approved" ? CheckCircle2 : CircleAlert;
  return (
    <div className="px-16 pt-16 md:px-24">
      <div className="flex flex-wrap items-center gap-x-12 gap-y-8">
        <Menu
          trigger={
            <Button
              size="sm"
              aria-label={`Version: ${shown?.label ?? view.label}`}
              icon={<ChevronDown aria-hidden="true" size={16} />}
            >
              {shown?.label ?? view.label}
            </Button>
          }
          items={tabs.map((tab) => ({
            label: tab.label,
            onSelect: () => {
              onSearch(tab.search);
            },
          }))}
        />
        {options === null ? null : (
          <nav aria-label="Change" className={SEGMENT_LIST}>
            {options.map((option) => (
              <HrefLink
                key={option.key}
                href={detailHref(slug, id, option.search)}
                aria-current={option.selected ? "page" : undefined}
                className={cn(segmentClasses(option.selected), focusRing)}
              >
                {option.label}
              </HrefLink>
            ))}
          </nav>
        )}
        <p className="flex min-w-0 flex-wrap items-center gap-x-8 text-14 text-text-2 md:ml-auto">
          <StateIcon
            aria-hidden="true"
            size={14}
            className={cn(
              "shrink-0",
              approval.tone === "approved"
                ? "text-ok"
                : approval.tone === "changes"
                  ? "text-bad"
                  : "text-text-2",
            )}
          />
          <span>
            <ActorText segments={approval.segments} />
            {checks === null ? "" : ` · ${checks}`} ·{" "}
            <HrefLink
              href={detailHref(slug, id, {})}
              className={cn("rounded-s text-accent hover:underline", focusRing)}
            >
              Overview
            </HrefLink>
          </span>
        </p>
      </div>
    </div>
  );
}

export interface ChangesTabProps {
  slug: string;
  proposal: ProposalDetail;
  comments: readonly ReviewComment[];
  questions: readonly LineQuestion[];
  view: DetailView;
  search: DetailSearch;
  human: string | null;
  now: number;
  runs: readonly CheckRun[] | null;
  onSearch: (next: DetailSearch) => void;
  onComment: (comment: LineComment) => Promise<void>;
  /** Resolves once the question is stored and the proposal's threads are read again. */
  onAsk: (question: LineComment) => Promise<void>;
  /** Resolves once the proposal with the human's new version is read again. */
  onEdited: (path: string) => Promise<void>;
}

function ChangeBody({
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
}: Omit<ChangesTabProps, "runs">) {
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
  const tree = useChangedFiles(
    viewedKey(slug, id, view.range),
    comparison.data ? shownPath(comparison.data, file?.path) : null,
  );
  const fileDiff = useFileDiff(
    proposalFileDiffQuery(slug, id, view.range, file?.path ?? ""),
    file !== null && fileBody(file) === "text",
  );
  const version = shownVersion(view);
  const fileText = useFileText(
    proposalFileQuery(slug, id, version, file?.path ?? ""),
    needsFileText(file, fileDiff),
  );
  const threads = buildThreads(comments, version);
  const byId = new Map(threads.map((t) => [t.id, t]));
  // Only the file on screen has its hunks read; a file whose text is not shown holds no line.
  const held = new Map<string, LineSet>();
  if (file !== null && fileBody(file) !== "text") {
    held.set(file.path, new Set());
  } else if (file !== null && fileDiff.status === "ready") {
    const diff = fileDiff.diff;
    const lines = heldFileLines(diff.hunks, diff.new_hash, fileText);
    if (lines !== null) held.set(file.path, lines);
  }
  const { placed, unplaced } = placeThreads(
    threads,
    files.map((f) => f.path),
    view.kind === "version",
    held,
  );
  const editable = canEdit(proposal, human, view) && editableFile(file);
  const shownVersionRow = proposal.versions.find((v) => v.version === version);
  const shownIntent =
    shownVersionRow === undefined ? null : versionIntent(shownVersionRow);
  const questionById = new Map(questions.map((q) => [q.id, q]));
  const requests = useQuery(requestsQuery(slug));
  const makeRequest = useCodeRequest(slug);
  const anchor: CodeAnchor = { kind: "version", proposal: id, version };
  const requestById = new Map((requests.data ?? []).map((r) => [r.id, r]));
  const marks = [
    ...questionMarks(questions, view),
    ...requestMarks(
      requests.data ?? [],
      anchor,
      commentsOnSide(view, "old") ? ["old", "new"] : ["new"],
    ),
  ];

  if (comparison.isPending) return <Skeleton className="m-24" />;
  if (comparison.isError) {
    return (
      <ErrorBanner
        className="m-24"
        title="The diff could not load."
        message="The comparison did not arrive."
        onRetry={() => void comparison.refetch()}
      />
    );
  }
  const data = comparison.data;
  if (editable && editing === file.path) {
    return (
      <div className="px-16 py-16 md:px-24">
        <FileEditor
          slug={slug}
          id={id}
          proposal={proposal}
          view={view}
          path={file.path}
          files={(proposal.selected.version === version
            ? proposal.files
            : files
          ).map((changedFile) => changedFile.path)}
          onCancel={() => {
            setEditing(null);
          }}
          onSaved={async () => {
            await onEdited(file.path);
          }}
        />
      </div>
    );
  }
  const showFile = (path: string) => {
    select(null);
    onSearch({ ...search, tab: "changes", path });
  };
  return (
    <div className="flex min-w-0 flex-col gap-16 px-16 pt-16 pb-28 md:px-24">
      {files.length === 0 ? (
        <EmptyPanel title="No changes">{emptyChangeText(view)}</EmptyPanel>
      ) : (
        <ChangedFiles
          state={tree}
          intent={shownIntent}
          comparison={data}
          selectedPath={file?.path}
          onSelectFile={showFile}
          fileDiff={fileDiff}
          fileText={fileText}
          selection={selection}
          onSelect={select}
          headAction={(path) =>
            editable && path === file.path ? (
              <Button
                size="sm"
                aria-label={`Edit ${file.path}`}
                icon={<Pencil aria-hidden="true" size={14} />}
                onClick={() => {
                  select(null);
                  setEditing(file.path);
                }}
              >
                Edit
              </Button>
            ) : null
          }
          threads={placed}
          marks={marks}
          renderMark={(mark, first) => {
            const question = questionById.get(mark.id);
            if (question) {
              return (
                <QuestionMarker
                  href={questionHref(slug, question)}
                  label={questionLabel(question)}
                  answered={question.answered}
                  focusable={first}
                />
              );
            }
            const request = requestById.get(mark.id);
            return request ? (
              <RequestMarker slug={slug} request={request} focusable={first} />
            ) : null;
          }}
          renderThread={(anchor) => {
            const thread = byId.get(anchor.id);
            return thread ? (
              <ThreadBox slug={slug} thread={thread} now={now} />
            ) : null;
          }}
          renderComposer={(sel, focusLine) => {
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
            if (mode === "request" && menu.request !== null) {
              return (
                <MakeRequestBox
                  slug={slug}
                  key={key}
                  title={makeRequestTitle(anchor, sel)}
                  onCancel={() => {
                    focusLine();
                    setMode("menu");
                  }}
                  onDone={() => {
                    focusLine();
                    select(null);
                  }}
                  onSubmit={(body) => makeRequest(anchor, sel, body)}
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
                onRequest={() => {
                  setMode("request");
                }}
              />
            );
          }}
        />
      )}
      {unplaced.length > 0 ? (
        <section aria-label="Other comments" className="flex flex-col gap-10">
          <h2 className="text-16 leading-tight font-semibold text-text">
            Other comments
          </h2>
          {unplaced.map((thread) => (
            <div
              key={thread.id}
              className="max-w-measure rounded-r-m border-l-3 border-accent bg-surface px-14 py-10"
            >
              <ThreadBox slug={slug} thread={thread} now={now} />
            </div>
          ))}
        </section>
      ) : null}
    </div>
  );
}

export function ChangesTab({ runs, ...body }: ChangesTabProps) {
  return (
    <div className="flex min-w-0 flex-col">
      <Toolbar
        slug={body.slug}
        proposal={body.proposal}
        view={body.view}
        runs={runs}
        onSearch={body.onSearch}
      />
      <ChangeBody
        key={`${String(body.view.range.version)}:${String(body.view.range.since)}`}
        {...body}
      />
    </div>
  );
}
