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
import { useQuery } from "@tanstack/react-query";
import { useId, useState, type ReactNode } from "react";
import { proposalVersionQuery } from "@/api/queries";
import type {
  CheckRun,
  GateStatus,
  Intent,
  ProposalDetail,
  ReviewComment,
  Verdict,
} from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { Chip } from "@/components/badge/Chip";
import type { ChipTone } from "@/components/badge/chips";
import { Id } from "@/components/id/Id";
import { CommitId } from "@/components/mono/CommitId";
import { Dialog } from "@/components/overlay/Dialog";
import { IdText } from "@/components/text/IdText";
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
import { conversationFact, type ConversationThread } from "./conversation";
import { changedFolders, changeTree, filesWord } from "./folderTree";
import { checklistGates, gateText, waitingSummary } from "./gates";
import {
  historyEntries,
  reviewCommentWhere,
  runSummary,
  type HistoryEntry,
  type HistoryReview,
} from "./history";
import { IdLink } from "@/components/id/IdLink";
import type { IntegrationView } from "./integration";
import {
  NO_INTENT,
  checkedBy,
  coverage,
  intentByline,
  intentCard,
  intentInHistory,
  versionIntent,
  type IntentCardModel,
} from "./intent";
import { IntentBody } from "./IntentView";
import {
  currentVersion,
  evidenceLines,
  evidenceSource,
  filesText,
  humanReviewFact,
} from "./present";
import { detailHref, type DetailSearch } from "./view";

const LABEL = "font-title text-13 leading-tight font-semibold text-text-2";
const CARD = "rounded-m bg-surface";
const LINK = cn("rounded-s text-accent hover:underline", focusRing);

const GATE_ICONS: Record<
  GateStatus,
  { icon: LucideIcon; label: string; className: string }
> = {
  met: { icon: CircleCheck, label: "met", className: "text-ok" },
  unmet: { icon: CircleX, label: "not met", className: "text-bad" },
  checked_at_integration: {
    icon: CircleDashed,
    label: "checked at integration",
    className: "text-text-2",
  },
};

const VERDICT_ICONS: Record<Verdict, { icon: LucideIcon; className: string }> =
  {
    approve: { icon: CircleCheck, className: "text-ok" },
    request_changes: { icon: CircleAlert, className: "text-bad" },
    reject: { icon: CircleSlash, className: "text-bad" },
    comment: { icon: MessageSquare, className: "text-text-2" },
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
        "flex max-w-measure items-start gap-10 rounded-s px-12 py-10 text-15 leading-normal text-text",
        tone === "danger" ? "bg-bad-soft" : "bg-sunken",
      )}
    >
      <Icon
        aria-hidden="true"
        size={16}
        className={cn(
          "mt-4 shrink-0",
          tone === "danger" ? "text-bad" : "text-text-2",
        )}
      />
      <span className="min-w-0">{children}</span>
    </p>
  );
}

function IntegrationCard({
  slug,
  proposal,
  now,
  main,
  integration,
  queued,
}: {
  slug: string;
  proposal: ProposalDetail;
  now: number;
  main: string | null;
  integration: IntegrationView | null;
  queued: string | null;
}) {
  const summary = proposal.summary;
  const waiting = waitingSummary(proposal);
  const checksNote = queued ?? checksFailedNote(summary);
  return (
    <section
      aria-label="Integration"
      className={cn(CARD, "flex flex-col gap-12 px-16 py-14")}
    >
      <div className="flex flex-wrap items-center gap-x-12 gap-y-6">
        <h2 className="text-18 leading-tight font-semibold text-text">
          Integration
        </h2>
        {integration ? (
          <p className="ml-auto flex flex-wrap items-center gap-x-6 text-13 text-text-2">
            <GitMerge aria-hidden="true" size={14} />v{integration.version}{" "}
            {integration.batch === null ? (
              "integrated as "
            ) : (
              <>
                integrated with batch{" "}
                <IdLink href={projectPath(slug, "batches", integration.batch)}>
                  {integration.batch}
                </IdLink>{" "}
                as{" "}
              </>
            )}
            <IdLink href={idHref(slug, "revision", integration.revision)}>
              {integration.revision}
            </IdLink>
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
      <ul className="flex flex-col gap-8">
        {checklistGates(proposal.gates).map((gate) => {
          const spec = GATE_ICONS[gate.status];
          const Icon = spec.icon;
          return (
            <li
              key={gate.gate}
              data-state={gate.status}
              className="flex max-w-measure gap-10"
            >
              <Icon
                role="img"
                aria-label={spec.label}
                size={16}
                className={cn("mt-4 shrink-0", spec.className)}
              />
              <span className="min-w-0 flex-1 text-15 leading-normal text-text">
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
    </section>
  );
}

/** How the current version works in the code: its summary, that every changed file belongs to a part, the parts with their files and the sections the intent has. A person reads it, with the reviewer's verdict beside it, instead of every line. */
function IntentCard({
  slug,
  proposal,
  model,
}: {
  slug: string;
  proposal: ProposalDetail;
  model: IntentCardModel;
}) {
  const checker = checkedBy(model);
  const byline = intentByline(model);
  const intent = model.intent;
  const covered =
    intent === null
      ? null
      : coverage(
          intent,
          proposal.files.map((file) => file.path),
        );
  return (
    <section
      aria-label="Intent"
      className={cn(CARD, "flex flex-col gap-14 rounded-l px-20 py-18")}
    >
      <div className="flex flex-wrap items-baseline justify-between gap-x-12 gap-y-4">
        <h2 className="text-16 leading-tight font-semibold text-text">
          Intent of v{model.version}
        </h2>
        {byline === null && checker === null ? null : (
          <span className="text-13 text-text-2">
            {byline === null ? null : byline.kind === "kept" ? (
              `kept from v${String(byline.from)}`
            ) : (
              <>
                {byline.words} <PersonName name={byline.by} />
              </>
            )}
            {checker === null ? null : (
              <>
                {byline === null ? "checked by " : " · checked by "}
                <PersonName name={checker} />
              </>
            )}
          </span>
        )}
      </div>
      {intent === null ? (
        <p className="text-15 leading-normal text-text-2">{NO_INTENT}</p>
      ) : (
        <>
          <MentionText
            text={intent.summary}
            slug={slug}
            className="max-w-measure text-16 leading-normal"
          />
          <p
            className={cn(
              "rounded-m px-14 py-10 text-14 text-text",
              covered !== null && covered.outside > 0
                ? "bg-warn-soft"
                : "bg-ok-soft",
            )}
          >
            {covered?.text}
          </p>
          <IntentBody
            slug={slug}
            id={proposal.summary.id}
            version={model.version}
            intent={intent}
            files={proposal.files}
          />
        </>
      )}
    </section>
  );
}

/** The reviewer's verdict on the current version and the summary of the review, beside the intent. */
function ReviewCard({
  slug,
  model,
  entry,
  now,
}: {
  slug: string;
  model: IntentCardModel;
  entry: HistoryEntry;
  now: number;
}) {
  const summary = model.summary;
  const spec = summary === null ? null : VERDICT_ICONS[summary.review.verdict];
  const Icon = spec?.icon;
  return (
    <section
      aria-label="Review"
      className={cn(CARD, "flex flex-col gap-10 px-16 py-14")}
    >
      <h2 className="text-16 leading-tight font-semibold text-text">Review</h2>
      {summary === null || spec === null || Icon === undefined ? (
        <p className="text-14 text-text-2">
          No reviewer has reviewed v{model.version} yet.
        </p>
      ) : (
        <div
          data-verdict={summary.review.verdict}
          className="flex min-w-0 flex-col gap-6"
        >
          <p className="flex flex-wrap items-center gap-x-8 text-14">
            <Icon
              aria-hidden="true"
              size={14}
              className={cn("shrink-0", spec.className)}
            />
            <span className={cn("font-semibold", spec.className)}>
              {summary.head}
            </span>
            <PersonName name={summary.review.reviewer} />
            <span className="text-13 text-text-2">
              {formatAsOfMinute(now, summary.review.created_at)}
            </span>
          </p>
          {summary.excerpt.text === "" ? null : (
            <MentionText
              text={summary.excerpt.text}
              slug={slug}
              className={cn(
                "text-14 leading-normal text-text",
                textFlow("lines"),
              )}
            />
          )}
          {summary.excerpt.cut ||
          summary.review.comments > 0 ||
          summary.comments.length > 0 ? (
            <FullReview slug={slug} entry={entry} item={summary} now={now} />
          ) : null}
        </div>
      )}
    </section>
  );
}

/** A version's intent in its History card: the summary, which opens to the whole intent. */
function HistoryIntent({
  slug,
  proposal,
  intent,
  version,
}: {
  slug: string;
  proposal: ProposalDetail;
  intent: Intent;
  version: number;
}) {
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  // The version's own changed files give its parts their line counts; they are read once the parts open.
  const read = useQuery({
    ...proposalVersionQuery(slug, proposal.summary.id, version),
    enabled: open,
  });
  return (
    <div className="flex min-w-0 flex-col gap-8">
      <MentionText
        text={intent.summary}
        slug={slug}
        className="max-w-measure text-15 leading-normal"
      />
      <button
        type="button"
        aria-expanded={open}
        aria-controls={open ? bodyId : undefined}
        className={cn("self-start text-14 font-semibold", LINK)}
        onClick={() => {
          setOpen((old) => !old);
        }}
      >
        {open
          ? "Hide the parts"
          : `Show the whole intent of v${String(version)}`}
      </button>
      {open ? (
        <div id={bodyId}>
          <IntentBody
            slug={slug}
            id={proposal.summary.id}
            version={version}
            intent={intent}
            files={read.data?.files ?? []}
          />
        </div>
      ) : null}
    </div>
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
    <li className="flex flex-col gap-4 rounded-s bg-sunken px-12 py-10">
      <span className="text-13 text-text-2">
        <Id>{comment.id}</Id> ·{" "}
        <span className="font-mono">{reviewCommentWhere(comment)}</span>
      </span>
      <MentionText
        text={comment.body}
        slug={slug}
        className="text-13 leading-normal"
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
      wide
      title={`${item.head} on v${String(entry.version.version)}`}
      trigger={
        <button
          type="button"
          className={cn("self-start text-14 font-semibold", LINK)}
        >
          Read the full review
        </button>
      }
    >
      <p className="text-13 text-text-2">
        <PersonName name={item.review.reviewer} /> ·{" "}
        {formatAsOfMinute(now, item.review.created_at)}
        {item.scope === null ? "" : ` · ${item.scope}`}
      </p>
      <MentionText
        text={item.review.body.trim() || item.head}
        slug={slug}
        className="text-15 leading-normal"
      />
      {item.comments.length === 0 ? null : (
        <section
          aria-label="Comments of the review"
          className="flex flex-col gap-8"
        >
          <h3 className={LABEL}>Comments</h3>
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
    <li className="flex flex-wrap items-center gap-x-8 gap-y-2 text-14">
      <Icon
        role="img"
        aria-label={spec.label}
        size={14}
        className={cn("shrink-0", spec.className)}
      />
      <IdLink
        href={detailHref(slug, id, {
          tab: "checks",
          run: run.id,
          ...(run.version === null ? {} : { version }),
        })}
      >
        {run.id}
      </IdLink>
      <span className="text-text-2">
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

const VERDICT_TONES: Record<string, ChipTone> = {
  approved: "ok",
  "changes requested": "bad",
  rejected: "bad",
};

/** A fact of a version: its label in the left column, the fact beside it. */
function VersionFact({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <>
      <dt className={cn(LABEL, "pt-2")}>{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  );
}

function VersionCard({
  slug,
  proposal,
  entry,
  now,
  showIntent,
}: {
  slug: string;
  proposal: ProposalDetail;
  entry: HistoryEntry;
  now: number;
  showIntent: boolean;
}) {
  const v = entry.version;
  const intent = versionIntent(v);
  const id = proposal.summary.id;
  return (
    <article
      aria-label={`Version v${String(v.version)}`}
      className={cn(CARD, "flex flex-col")}
    >
      <header className="flex flex-wrap items-center gap-x-10 gap-y-6 px-16 pt-14 pb-4">
        <h3 className="font-title text-16 font-semibold text-text tabular-nums">
          v{v.version}
        </h3>
        <Chip tone={VERDICT_TONES[entry.verdict] ?? "neutral"}>
          {entry.verdict}
        </Chip>
        <span className="min-w-0 flex-1 text-13 text-text-2 max-sm:order-last max-sm:basis-full">
          {ORIGIN_WORDS[entry.origin]} <PersonName name={v.created_by} />
          {entry.mergedMain === null ? (
            ""
          ) : (
            <>
              {" "}
              · merge of main <IdText text={entry.mergedMain} />
            </>
          )}
          {v.review_again ? " · sent for review again" : ""} ·{" "}
          {formatAsOfMinute(now, v.created_at)} ·{" "}
          <IdText text={`${v.changeset} on ${v.base}`} />
        </span>
        <HrefLink
          href={detailHref(slug, id, entry.diff.search)}
          className={cn("text-14 max-sm:ml-auto", LINK)}
        >
          {entry.diff.label}
        </HrefLink>
      </header>
      <dl className="grid grid-cols-[var(--width-fact-label)_minmax(0,1fr)] gap-x-14 gap-y-10 px-16 pt-8 pb-14 text-14">
        {showIntent ? (
          <VersionFact label="Intent">
            {intent === null ? (
              <p className="text-text-2">{NO_INTENT}</p>
            ) : (
              <HistoryIntent
                slug={slug}
                proposal={proposal}
                intent={intent}
                version={v.version}
              />
            )}
          </VersionFact>
        ) : null}
        <VersionFact label="Checks">
          {entry.runsFailed ? (
            <p role="alert" className="font-medium text-bad">
              The checks could not be read.
            </p>
          ) : entry.runs === null ? (
            <p className="text-text-2">Reading the checks…</p>
          ) : entry.runs.length === 0 ? (
            <p className="text-text-2">fold ran no check on this version.</p>
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
        </VersionFact>
        {entry.reviews.map((item, index) => {
          const spec = VERDICT_ICONS[item.review.verdict];
          const Icon = spec.icon;
          return (
            <VersionFact key={index} label="Review">
              <div
                data-verdict={item.review.verdict}
                className="flex min-w-0 flex-col gap-4"
              >
                <p className="flex flex-wrap items-center gap-x-8">
                  <Icon
                    aria-hidden="true"
                    size={14}
                    className={cn("shrink-0", spec.className)}
                  />
                  <span className={cn("font-semibold", spec.className)}>
                    {item.head}
                  </span>
                  <PersonName name={item.review.reviewer} />
                  <span className="text-13 text-text-2">
                    {item.scope === null ? "" : `${item.scope} · `}
                    {formatAsOfMinute(now, item.review.created_at)}
                  </span>
                </p>
                {item.excerpt.text === "" ? null : (
                  <MentionText
                    text={item.excerpt.text}
                    slug={slug}
                    className={cn(
                      "max-w-measure text-15 leading-normal text-text",
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
            </VersionFact>
          );
        })}
        {v.evidence ? (
          <>
            <dt />
            <dd className="min-w-0">
              <details className="group flex flex-col gap-6">
                <summary
                  className={cn(
                    "cursor-pointer self-start text-14 font-semibold text-accent",
                    focusRing,
                  )}
                >
                  Evidence from {evidenceSource(v)}
                </summary>
                <pre
                  className={cn(
                    "mt-8 flex min-w-0 flex-col gap-2 rounded-s bg-sunken px-12 py-10 font-mono text-13 leading-normal",
                    textFlow("prose"),
                  )}
                >
                  {evidenceLines(v.evidence).map((line, index) => (
                    <span
                      key={index}
                      className={line.command ? "text-text" : "text-text-2"}
                    >
                      <MentionInline text={line.text} slug={slug} />
                    </span>
                  ))}
                </pre>
              </details>
            </dd>
          </>
        ) : null}
      </dl>
    </article>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="pt-2 text-13 text-text-2">{label}</dt>
      <dd className="min-w-0 text-text">{children}</dd>
    </>
  );
}

function AboutCard({
  slug,
  proposal,
  main,
  area,
  threads,
  onTab,
  review,
}: {
  slug: string;
  proposal: ProposalDetail;
  main: string | null;
  area: string | null;
  threads: readonly ConversationThread[] | null;
  onTab: (search: DetailSearch) => string;
  /** The reviewer's verdict on the current version, drawn above the facts. */
  review: ReactNode;
}) {
  const summary = proposal.summary;
  const current = currentVersion(proposal);
  const folders = changedFolders(changeTree(proposal.files));
  const talk = threads === null ? null : conversationFact(threads);
  const mark = summary.human_review;
  return (
    <aside className="flex min-w-0 flex-col gap-20 lg:w-side-panel lg:shrink-0">
      {review}
      <section
        aria-label="About"
        className={cn(CARD, "flex flex-col gap-12 px-16 py-14")}
      >
        <h2 className="text-16 leading-tight font-semibold text-text">About</h2>
        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-16 gap-y-8 text-14">
          <Fact label="Author">
            <PersonName name={summary.author} />
          </Fact>
          <Fact label="Task">
            <IdLink href={projectPath(slug, "t", summary.task)}>
              {summary.task}
            </IdLink>{" "}
            · {proposal.task.title}
          </Fact>
          {area === null ? null : <Fact label="Area">{area}</Fact>}
          {current ? (
            <Fact label="Base">
              <IdText
                text={main ? `${current.base} → main ${main}` : current.base}
              />
            </Fact>
          ) : null}
          {current ? (
            <Fact label="Size">{filesText(current, proposal.totals)}</Fact>
          ) : null}
          {summary.state === "open" ? (
            <Fact label="Review claim">
              {summary.review_claim === null ? (
                <span className="text-text-2">not claimed</span>
              ) : (
                <PersonName name={summary.review_claim} />
              )}
            </Fact>
          ) : null}
          {mark === null ? null : (
            <Fact label="Human review">
              <ActorText segments={humanReviewFact(mark)} />
            </Fact>
          )}
          {proposal.prerequisites.length > 0 ? (
            <Fact label="Needs">
              {proposal.prerequisites.map((t, index) => (
                <span key={t.task}>
                  {index > 0 ? ", " : ""}
                  <IdLink href={projectPath(slug, "t", t.task)}>
                    {t.task}
                  </IdLink>{" "}
                  ({t.status})
                </span>
              ))}
            </Fact>
          ) : null}
          <Fact label="Conversation">
            {talk === null ? (
              <span className="text-text-2">
                {threads === null ? "Reading the threads…" : "No thread yet."}
              </span>
            ) : (
              <>
                <HrefLink
                  href={onTab({ tab: "conversation" })}
                  className={LINK}
                >
                  {talk.threads}
                </HrefLink>
                {talk.waiting === null ? null : (
                  <>
                    {" "}
                    · <span className="text-warn">{talk.waiting}</span>
                  </>
                )}
              </>
            )}
          </Fact>
        </dl>
        <div aria-hidden="true" className="my-4 h-px bg-line" />
        <section aria-label="Changed folders" className="flex flex-col gap-8">
          <div className="flex items-baseline justify-between gap-8">
            <h3 className={LABEL}>Changed folders</h3>
            <HrefLink
              href={onTab({ tab: "changes" })}
              className={cn("text-13", LINK)}
            >
              Open changes
            </HrefLink>
          </div>
          {folders.length === 0 ? (
            <p className="text-13 text-text-2">
              The current version changes no file.
            </p>
          ) : (
            <ul className="flex flex-col gap-4">
              {folders.map((folder) => (
                <li key={folder.path}>
                  <HrefLink
                    href={onTab({ tab: "changes", path: folder.first })}
                    className={cn(
                      "-mx-6 flex items-center gap-10 rounded-s px-6 py-4 text-13 hover:bg-sunken",
                      focusRing,
                    )}
                  >
                    <span className="min-w-0 flex-1 truncate font-mono text-13 text-text">
                      {folder.path === "" ? "project root" : folder.path}
                    </span>
                    <span className="shrink-0 text-text-2">
                      {filesWord(folder.totals.files)}
                    </span>
                    <span className="shrink-0 font-mono text-ok">
                      +{folder.totals.added}
                    </span>
                    <span className="shrink-0 font-mono text-bad">
                      −{folder.totals.removed}
                    </span>
                  </HrefLink>
                </li>
              ))}
            </ul>
          )}
        </section>
      </section>
    </aside>
  );
}

export interface OverviewTabProps {
  slug: string;
  proposal: ProposalDetail;
  now: number;
  main: string | null;
  integration: IntegrationView | null;
  queued: string | null;
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
  now,
  main,
  integration,
  queued,
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
  const intent = intentCard(entries);
  const currentEntry = entries.find((entry) => entry.current) ?? entries[0];
  return (
    <div className="flex min-w-0 flex-col gap-20 px-16 pt-20 pb-28 md:px-24 lg:flex-row lg:items-start">
      <div className="flex min-w-0 flex-1 flex-col gap-20">
        {intent === null ? null : (
          <IntentCard slug={slug} proposal={proposal} model={intent} />
        )}
        <IntegrationCard
          slug={slug}
          proposal={proposal}
          now={now}
          main={main}
          integration={integration}
          queued={queued}
        />
        <section aria-label="History" className="flex flex-col gap-12">
          <div className="flex flex-wrap items-baseline gap-x-12">
            <h2 className="text-18 leading-tight font-semibold text-text">
              History
            </h2>
            <span className="text-13 text-text-2">
              Newest first. Each version with its checks and reviews.
            </span>
          </div>
          {entries.map((entry) => (
            <VersionCard
              key={entry.version.version}
              slug={slug}
              proposal={proposal}
              entry={entry}
              now={now}
              showIntent={intentInHistory(entry, intent)}
            />
          ))}
        </section>
      </div>
      <AboutCard
        slug={slug}
        proposal={proposal}
        main={main}
        area={area}
        threads={threads}
        onTab={tabHref}
        review={
          intent === null || currentEntry === undefined ? null : (
            <ReviewCard
              slug={slug}
              model={intent}
              entry={currentEntry}
              now={now}
            />
          )
        }
      />
    </div>
  );
}
