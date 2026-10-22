import {
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleSlash,
  CircleX,
  GitMerge,
  Hourglass,
  ListChecks,
  MessageSquare,
  type LucideIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import type {
  CheckRun,
  GateStatus,
  ProposalDetail,
  ReviewComment,
  Verdict,
} from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { CommitId } from "@/components/mono/CommitId";
import { Dialog } from "@/components/overlay/Dialog";
import { textFlow } from "@/components/text/textFlow";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionInline, MentionText } from "@/mentions/MentionText";
import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import { idHref, projectPath } from "@/routes/paths";
import { RUN_ICONS } from "./checkIcons";
import { checksFailedNote } from "./checkRuns";
import { ConflictBlock } from "./ConflictBlock";
import {
  lastLine,
  latestThreads,
  whereText,
  type ConversationThread,
} from "./conversation";
import { changedFolders, changeTree, filesWord } from "./folderTree";
import { gateText, shownGates, waitingSummary } from "./gates";
import {
  historyEntries,
  reviewCommentWhere,
  runSummary,
  type HistoryEntry,
  type HistoryReview,
} from "./history";
import { HumanActions, type ActionHandlers } from "./HumanActions";
import type { IntegrationView } from "./integration";
import {
  currentVersion,
  evidenceLines,
  evidenceSource,
  filesText,
  PILL_CLASSES,
} from "./present";
import { detailHref, type DetailSearch, type DetailView } from "./view";

const OVERLINE =
  "text-overline leading-snug font-semibold text-text-muted uppercase";
const CARD = "rounded-l bg-surface inset-ring inset-ring-border-subtle";
const LINK = cn("rounded-s text-primary-text hover:underline", focusRing);

const GATE_ICONS: Record<
  GateStatus,
  { icon: LucideIcon; label: string; className: string }
> = {
  met: { icon: CircleCheck, label: "met", className: "text-primary-text" },
  unmet: { icon: CircleX, label: "not met", className: "text-danger" },
  checked_at_integration: {
    icon: CircleDashed,
    label: "checked at integration",
    className: "text-text-muted",
  },
};

const VERDICT_ICONS: Record<Verdict, { icon: LucideIcon; className: string }> =
  {
    approve: { icon: CircleCheck, className: "text-primary-text" },
    request_changes: { icon: CircleAlert, className: "text-danger" },
    reject: { icon: CircleSlash, className: "text-danger" },
    comment: { icon: MessageSquare, className: "text-text-muted" },
  };

function Notice({
  icon: Icon,
  tone,
  children,
}: {
  icon: LucideIcon;
  tone: "muted" | "danger";
  children: ReactNode;
}) {
  return (
    <p
      className={cn(
        "flex items-start gap-8 rounded-m px-12 py-9 text-code leading-snug",
        tone === "danger" && "bg-danger-soft text-danger",
        tone === "muted" && "bg-sunken text-text-muted",
      )}
    >
      <Icon aria-hidden="true" size={14} className="mt-2 shrink-0" />
      <span className="min-w-0">{children}</span>
    </p>
  );
}

function IntegrationCard({
  slug,
  proposal,
  human,
  now,
  main,
  integration,
  queued,
  actions,
  actionView,
}: {
  slug: string;
  proposal: ProposalDetail;
  human: string | null;
  now: number;
  main: string | null;
  integration: IntegrationView | null;
  queued: string | null;
  actions: ActionHandlers;
  actionView: DetailView;
}) {
  const summary = proposal.summary;
  const waiting = waitingSummary(proposal);
  const checksNote = queued ?? checksFailedNote(summary);
  return (
    <section
      aria-label="Integration"
      className={cn(CARD, "flex flex-col gap-14 p-20")}
    >
      <div className="flex flex-wrap items-center gap-x-12 gap-y-6">
        <h2 className="text-section leading-snug font-semibold text-text">
          Integration
        </h2>
        {integration ? (
          <p className="ml-auto flex flex-wrap items-center gap-x-6 text-code text-text-muted">
            <GitMerge aria-hidden="true" size={14} />v{integration.version}{" "}
            {integration.batch === null ? (
              "integrated as "
            ) : (
              <>
                integrated with batch{" "}
                <HrefLink
                  href={projectPath(slug, "batches", integration.batch)}
                  className={cn("font-mono", LINK)}
                >
                  {integration.batch}
                </HrefLink>{" "}
                as{" "}
              </>
            )}
            <HrefLink
              href={idHref(slug, "revision", integration.revision)}
              className={cn("font-mono", LINK)}
            >
              {integration.revision}
            </HrefLink>
            <CommitId commit={integration.commit} />
            <span>· {formatAsOfMinute(now, integration.at)}</span>
            {integration.actor ? (
              <span>
                · by <PersonName name={integration.actor} />
              </span>
            ) : null}
          </p>
        ) : null}
      </div>
      <ul className="grid grid-cols-1 gap-x-24 gap-y-10 md:grid-cols-2">
        {shownGates(proposal.gates).map((gate) => {
          const spec = GATE_ICONS[gate.status];
          const Icon = spec.icon;
          return (
            <li key={gate.gate} data-state={gate.status} className="flex gap-8">
              <Icon
                role="img"
                aria-label={spec.label}
                size={15}
                className={cn("mt-2 shrink-0", spec.className)}
              />
              <span className="min-w-0 flex-1 text-bar leading-snug text-text">
                <ActorText
                  segments={gateText(gate, proposal, main)}
                  slot="gate"
                />
              </span>
            </li>
          );
        })}
      </ul>
      {waiting ? (
        <Notice icon={Hourglass} tone="muted">
          <ActorText segments={waiting} slot="waitingLine" />
        </Notice>
      ) : null}
      {checksNote ? (
        <Notice
          icon={ListChecks}
          tone={summary.checks_failed ? "danger" : "muted"}
        >
          <span data-checks-failed={summary.checks_failed}>{checksNote}</span>
        </Notice>
      ) : null}
      {proposal.conflict ? (
        <ConflictBlock report={proposal.conflict} proposal={proposal} />
      ) : null}
      {human !== null && summary.state === "open" ? (
        <HumanActions
          proposal={proposal}
          human={human}
          view={actionView}
          slug={slug}
          actions={actions}
        />
      ) : null}
    </section>
  );
}

function ReviewCommentItem({
  slug,
  comment,
}: {
  slug: string;
  comment: ReviewComment;
}) {
  return (
    <li className="flex flex-col gap-4 rounded-m bg-sunken px-12 py-9">
      <span className="font-mono text-caption text-text-muted">
        {comment.id} · {reviewCommentWhere(comment)}
      </span>
      <MentionText
        text={comment.body}
        slug={slug}
        className="text-code leading-normal"
      />
    </li>
  );
}

function FullReview({
  slug,
  entry,
  item,
  now,
}: {
  slug: string;
  entry: HistoryEntry;
  item: HistoryReview;
  now: number;
}) {
  return (
    <Dialog
      title={`${item.head} on v${String(entry.version.version)}`}
      trigger={
        <button
          type="button"
          className={cn("self-start text-meta font-semibold", LINK)}
        >
          Read the full review
        </button>
      }
    >
      <div className="flex w-dialog max-w-full flex-col gap-10">
        <p className="text-code text-text-muted">
          <PersonName name={item.review.reviewer} /> ·{" "}
          {formatAsOfMinute(now, item.review.created_at)}
          {item.scope === null ? "" : ` · ${item.scope}`}
        </p>
        <div className="flex max-h-review-dialog flex-col gap-14 overflow-y-auto">
          <MentionText
            text={item.review.body.trim() || item.head}
            slug={slug}
            className="text-body leading-normal"
          />
          {item.comments.length === 0 ? null : (
            <section
              aria-label="Comments of the review"
              className="flex flex-col gap-8"
            >
              <h3 className={OVERLINE}>Comments</h3>
              <ul className="flex flex-col gap-8">
                {item.comments.map((comment) => (
                  <ReviewCommentItem
                    key={comment.id}
                    slug={slug}
                    comment={comment}
                  />
                ))}
              </ul>
            </section>
          )}
        </div>
      </div>
    </Dialog>
  );
}

function RunLine({
  slug,
  id,
  run,
  version,
}: {
  slug: string;
  id: string;
  run: CheckRun;
  version: number;
}) {
  const spec = RUN_ICONS[run.state];
  const Icon = spec.icon;
  return (
    <li className="flex flex-wrap items-center gap-x-8 gap-y-2 text-code">
      <Icon
        role="img"
        aria-label={spec.label}
        size={14}
        className={cn("shrink-0", spec.className)}
      />
      <HrefLink
        href={detailHref(slug, id, {
          tab: "checks",
          run: run.id,
          ...(run.version === null ? {} : { version }),
        })}
        className={cn("font-mono text-meta", LINK)}
      >
        {run.id}
      </HrefLink>
      <span className="text-text-muted">
        {run.stage === "integration" ? `integration on ${run.base} · ` : ""}
        {runSummary(run)}
      </span>
    </li>
  );
}

const ORIGIN_WORDS: Record<HistoryEntry["origin"], string> = {
  author: "by",
  review_fix: "review fix by",
  human_edit: "human edit by",
};

function VersionCard({
  slug,
  proposal,
  entry,
  now,
}: {
  slug: string;
  proposal: ProposalDetail;
  entry: HistoryEntry;
  now: number;
}) {
  const v = entry.version;
  const id = proposal.summary.id;
  return (
    <article
      aria-label={`Version v${String(v.version)}`}
      className={cn(CARD, "flex flex-col")}
    >
      <header className="flex flex-wrap items-center gap-x-12 gap-y-6 border-b border-border-subtle px-18 py-12">
        <h3 className="font-mono text-body font-semibold text-text">
          v{v.version}
        </h3>
        <span
          className={cn(
            "rounded-s px-8 py-2 text-caption font-semibold",
            entry.verdict === "approved"
              ? PILL_CLASSES.approved
              : entry.verdict === "changes requested" ||
                  entry.verdict === "rejected"
                ? PILL_CLASSES.danger
                : PILL_CLASSES.neutral,
          )}
        >
          {entry.verdict}
        </span>
        <span className="text-code text-text-muted">
          {ORIGIN_WORDS[entry.origin]} <PersonName name={v.created_by} />
          {entry.mergedMain === null
            ? ""
            : ` · merge of main ${entry.mergedMain}`}
          {v.review_again ? " · sent for review again" : ""} ·{" "}
          {formatAsOfMinute(now, v.created_at)} ·{" "}
          <span className="font-mono text-meta">
            {v.changeset} on {v.base}
          </span>
        </span>
        <HrefLink
          href={detailHref(slug, id, entry.diff.search)}
          className={cn("ml-auto text-code", LINK)}
        >
          {entry.diff.label}
        </HrefLink>
      </header>
      <div className="flex flex-col gap-12 px-18 py-12">
        <div className="flex gap-10">
          <span className="w-fact-label shrink-0 text-code text-text-faint">
            Checks
          </span>
          {entry.runsFailed ? (
            <p role="alert" className="text-code font-medium text-danger">
              The checks could not be read.
            </p>
          ) : entry.runs === null ? (
            <p className="text-code text-text-muted">Reading the checks…</p>
          ) : entry.runs.length === 0 ? (
            <p className="text-code text-text-muted">
              fold ran no check on this version.
            </p>
          ) : (
            <ul className="flex min-w-0 flex-col gap-4">
              {entry.runs.map((run) => (
                <RunLine
                  key={run.id}
                  slug={slug}
                  id={id}
                  run={run}
                  version={v.version}
                />
              ))}
            </ul>
          )}
        </div>
        {entry.reviews.map((item, index) => {
          const spec = VERDICT_ICONS[item.review.verdict];
          const Icon = spec.icon;
          return (
            <div
              key={index}
              data-verdict={item.review.verdict}
              className="flex gap-10"
            >
              <span className="w-fact-label shrink-0 text-code text-text-faint">
                Review
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-2">
                <p className="flex flex-wrap items-center gap-x-6 text-code">
                  <Icon
                    aria-hidden="true"
                    size={14}
                    className={cn("shrink-0", spec.className)}
                  />
                  <span className={cn("font-semibold", spec.className)}>
                    {item.head}
                  </span>
                  <PersonName name={item.review.reviewer} />
                  <span className="text-text-faint">
                    {item.scope === null ? "" : `· ${item.scope} `}·{" "}
                    {formatAsOfMinute(now, item.review.created_at)}
                  </span>
                </p>
                {item.excerpt.text === "" ? null : (
                  <MentionText
                    text={item.excerpt.text}
                    slug={slug}
                    className={cn(
                      "text-code leading-normal text-text-muted",
                      textFlow("lines"),
                    )}
                  />
                )}
                {item.excerpt.cut ||
                item.review.comments > 0 ||
                item.comments.length > 0 ? (
                  <FullReview slug={slug} entry={entry} item={item} now={now} />
                ) : null}
              </div>
            </div>
          );
        })}
        {v.evidence ? (
          <details className="group flex flex-col gap-6">
            <summary
              className={cn(
                "cursor-pointer self-start text-code font-semibold text-primary-text",
                focusRing,
              )}
            >
              Evidence from {evidenceSource(v)}
            </summary>
            <pre
              className={cn(
                "mt-6 flex min-w-0 flex-col gap-2 rounded-m bg-sunken px-12 py-10 font-mono text-label",
                textFlow("prose"),
              )}
            >
              {evidenceLines(v.evidence).map((line, index) => (
                <span
                  key={index}
                  className={line.command ? "text-text" : "text-text-muted"}
                >
                  <MentionInline text={line.text} slug={slug} />
                </span>
              ))}
            </pre>
          </details>
        ) : null}
      </div>
    </article>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex gap-12">
      <dt className="w-fact-label shrink-0 text-text-faint">{label}</dt>
      <dd className="min-w-0 flex-1 text-text">{children}</dd>
    </div>
  );
}

function SideColumn({
  slug,
  proposal,
  main,
  area,
  threads,
  onTab,
}: {
  slug: string;
  proposal: ProposalDetail;
  main: string | null;
  area: string | null;
  threads: readonly ConversationThread[] | null;
  onTab: (search: DetailSearch) => string;
}) {
  const summary = proposal.summary;
  const current = currentVersion(proposal);
  const folders = changedFolders(changeTree(proposal.files));
  const latest = threads === null ? null : latestThreads(threads);
  return (
    <aside className="flex min-w-0 flex-col gap-20 lg:w-side-panel lg:shrink-0">
      <section
        aria-label="About"
        className={cn(CARD, "flex flex-col gap-12 p-18")}
      >
        <h2 className={OVERLINE}>About</h2>
        <dl className="flex flex-col gap-8 text-code">
          <Fact label="Author">
            <PersonName name={summary.author} />
          </Fact>
          <Fact label="Task">
            <HrefLink
              href={projectPath(slug, "t", summary.task)}
              className={cn("font-mono", LINK)}
            >
              {summary.task}
            </HrefLink>{" "}
            · {proposal.task.title}
          </Fact>
          {area === null ? null : <Fact label="Area">{area}</Fact>}
          {current ? (
            <Fact label="Base">
              <span className="font-mono">
                {current.base}
                {main ? ` → main ${main}` : ""}
              </span>
            </Fact>
          ) : null}
          {current ? (
            <Fact label="Size">{filesText(current, proposal.totals)}</Fact>
          ) : null}
          {proposal.prerequisites.length > 0 ? (
            <Fact label="Needs">
              {proposal.prerequisites.map((t, index) => (
                <span key={t.task}>
                  {index > 0 ? ", " : ""}
                  <HrefLink
                    href={projectPath(slug, "t", t.task)}
                    className={cn("font-mono", LINK)}
                  >
                    {t.task}
                  </HrefLink>{" "}
                  ({t.status})
                </span>
              ))}
            </Fact>
          ) : null}
        </dl>
      </section>
      <section
        aria-label="Changed folders"
        className={cn(CARD, "flex flex-col gap-10 p-18")}
      >
        <div className="flex items-center gap-8">
          <h2 className={OVERLINE}>Changed folders</h2>
          <HrefLink
            href={onTab({ tab: "changes" })}
            className={cn("ml-auto text-meta", LINK)}
          >
            Open changes
          </HrefLink>
        </div>
        {folders.length === 0 ? (
          <p className="text-code text-text-muted">
            The current version changes no file.
          </p>
        ) : (
          <ul className="flex flex-col gap-6">
            {folders.map((folder) => (
              <li key={folder.path}>
                <HrefLink
                  href={onTab({ tab: "changes", path: folder.first })}
                  className={cn(
                    "flex items-center gap-10 rounded-s text-code hover:bg-sunken",
                    focusRing,
                  )}
                >
                  <span className="min-w-0 flex-1 truncate font-mono text-meta text-text">
                    {folder.path === "" ? "project root" : folder.path}
                  </span>
                  <span className="shrink-0 text-meta text-text-faint">
                    {filesWord(folder.totals.files)}
                  </span>
                  <span className="shrink-0 font-mono text-meta text-primary-text">
                    +{folder.totals.added}
                  </span>
                  <span className="shrink-0 font-mono text-meta text-danger">
                    −{folder.totals.removed}
                  </span>
                </HrefLink>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section
        aria-label="Latest threads"
        className={cn(CARD, "flex flex-col gap-10 p-18")}
      >
        <div className="flex items-center gap-8">
          <h2 className={OVERLINE}>Conversation</h2>
          <HrefLink
            href={onTab({ tab: "conversation" })}
            className={cn("ml-auto text-meta", LINK)}
          >
            Open the conversation
          </HrefLink>
        </div>
        {latest === null ? (
          <p className="text-code text-text-muted">Reading the threads…</p>
        ) : latest.length === 0 ? (
          <p className="text-code text-text-muted">No thread yet.</p>
        ) : (
          <ul className="flex flex-col">
            {latest.map((thread) => {
              const line = lastLine(thread);
              return (
                <li
                  key={thread.key}
                  className="flex flex-col gap-2 border-t border-border-subtle py-8 text-code"
                >
                  <span className="truncate font-mono text-caption text-text-faint">
                    {whereText(thread)}
                  </span>
                  <span className="min-w-0 text-text">
                    {line.author === "" ? null : (
                      <>
                        <PersonName name={line.author} />:{" "}
                      </>
                    )}
                    <MentionInline text={line.text} slug={slug} /> ·{" "}
                    <span className="text-text-muted">{thread.stateText}</span>
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </section>
    </aside>
  );
}

export interface OverviewTabProps {
  slug: string;
  proposal: ProposalDetail;
  human: string | null;
  now: number;
  main: string | null;
  integration: IntegrationView | null;
  queued: string | null;
  actions: ActionHandlers;
  /** The current version, which the human actions act on. */
  actionView: DetailView;
  runs: ReadonlyMap<number, readonly CheckRun[]>;
  /** The versions whose read of their runs failed. */
  failedVersions: ReadonlySet<number>;
  /** Every review comment of the proposal; empty until they arrive. */
  comments: readonly ReviewComment[];
  threads: readonly ConversationThread[] | null;
  area: string | null;
  /** The address of another tab of this proposal. */
  tabHref: (search: DetailSearch) => string;
}

export function OverviewTab({
  slug,
  proposal,
  human,
  now,
  main,
  integration,
  queued,
  actions,
  actionView,
  runs,
  failedVersions,
  comments,
  threads,
  area,
  tabHref,
}: OverviewTabProps) {
  const entries = historyEntries(proposal, runs, {
    comments,
    failed: failedVersions,
  });
  return (
    <div className="flex min-w-0 flex-col gap-24 px-16 py-20 md:px-24 lg:flex-row lg:items-start">
      <div className="flex min-w-0 flex-1 flex-col gap-24">
        <IntegrationCard
          slug={slug}
          proposal={proposal}
          human={human}
          now={now}
          main={main}
          integration={integration}
          queued={queued}
          actions={actions}
          actionView={actionView}
        />
        <section aria-label="History" className="flex flex-col gap-12">
          <div className="flex flex-wrap items-baseline gap-x-12">
            <h2 className={OVERLINE}>History</h2>
            <span className="text-meta text-text-faint">
              newest first · each version with its checks and reviews
            </span>
          </div>
          {entries.map((entry) => (
            <VersionCard
              key={entry.version.version}
              slug={slug}
              proposal={proposal}
              entry={entry}
              now={now}
            />
          ))}
        </section>
      </div>
      <SideColumn
        slug={slug}
        proposal={proposal}
        main={main}
        area={area}
        threads={threads}
        onTab={tabHref}
      />
    </div>
  );
}
