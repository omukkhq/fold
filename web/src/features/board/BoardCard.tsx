import { skipToken, useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import {
  ClipboardCheck,
  GitFork,
  GitMerge,
  GitPullRequest,
  Hand,
  Hourglass,
  ListChecks,
  MessageSquare,
  UserRound,
  type LucideIcon,
} from "lucide-react";
import type { BoardCard as BoardCardData, BoardProposal } from "@/api/board";
import { keys } from "@/api/keys";
import { HrefLink } from "@/app/HrefLink";
import { Badge } from "@/components/badge/Badge";
import { Card } from "@/components/card/Card";
import { Label } from "@/components/label/Label";
import { Mono } from "@/components/mono/Mono";
import { Tooltip } from "@/components/overlay/Tooltip";
import { cn } from "@/lib/cn";
import { HumanReviewBadge } from "@/people/HumanReviewBadge";
import { PersonName } from "@/people/PersonName";
import type { LiveWorkspace } from "@/live/envelope";
import { activityLine, liveFilesText, type ActivityLine } from "./activityLine";
import { MarkAnswered } from "@/features/messages/MarkAnswered";
import { taskCanMarkAnswered } from "@/features/messages/answered";
import { attentionMark, hasKind } from "./attention";
import {
  blockedText,
  blockerMark,
  blocksText,
  checklistText,
  kindText,
  leadCheckMark,
  messagesText,
  needsText,
  planWaitText,
  proposalLook,
  proposalText,
  taskPath,
  type BlockerMark,
  type LeadCheckMark,
  type ProposalLook,
} from "./card";

const PROPOSAL_ICON: Record<ProposalLook, { icon: LucideIcon; color: string }> =
  {
    attention: { icon: GitPullRequest, color: "text-danger" },
    active: { icon: GitPullRequest, color: "text-primary-text" },
    merged: { icon: GitMerge, color: "text-text-muted" },
  };

function Meta({
  icon: Icon,
  text,
  label,
}: {
  icon: LucideIcon;
  text: string;
  /** Names a bare number for a screen reader. */
  label?: string;
}) {
  return (
    <span className="inline-flex items-center gap-4 text-meta text-text-muted">
      <Icon aria-hidden="true" size={13} />
      {text}
      {label ? <span className="sr-only">{label}</span> : null}
    </span>
  );
}

/** The proposal pill; a proposal that waits in a review plan says under it what its batch waits for. */
function ProposalPill({
  proposal,
  now,
}: {
  proposal: BoardProposal;
  now: number;
}) {
  const { icon: Icon, color } = PROPOSAL_ICON[proposalLook(proposal)];
  const wait = planWaitText(proposal, now);
  const pill = (
    <span className="inline-flex items-center gap-4 self-start rounded-s bg-surface px-7 py-2 text-meta font-medium text-text inset-ring inset-ring-border">
      <Icon aria-hidden="true" size={12} className={color} />
      {proposalText(proposal)}
    </span>
  );
  if (wait === null) {
    return pill;
  }
  return (
    <div className="flex flex-col gap-4">
      {pill}
      <p className="flex items-start gap-4 text-meta leading-normal text-text-muted">
        <Hourglass aria-hidden="true" size={12} className="mt-2 shrink-0" />
        <span className="min-w-0">{wait}</span>
      </p>
    </div>
  );
}

/** The proposal pill's place and shape, for a reported task without a proposal: the mark and the task worker's note. */
function LeadCheckPill({ mark }: { mark: LeadCheckMark }) {
  return (
    <div className="flex flex-col gap-4">
      <span className="inline-flex items-center gap-4 self-start rounded-s bg-surface px-7 py-2 text-meta font-medium text-text inset-ring inset-ring-border">
        <ClipboardCheck
          aria-hidden="true"
          size={12}
          className="text-secondary-text"
        />
        {mark.text}
      </span>
      <p className="line-clamp-2 text-meta leading-normal text-text-muted">
        {mark.note}
      </p>
    </div>
  );
}

/** The blocker under the title: agents may not take the task until a human clears it. */
function BlockerPill({ mark }: { mark: BlockerMark }) {
  return (
    <div className="flex flex-col gap-4">
      <span className="inline-flex items-center gap-5 self-start rounded-s bg-warning-soft px-8 py-3 text-caption font-semibold text-warning">
        <Hand aria-hidden="true" size={12} />
        {mark.text}
      </span>
      <p className="line-clamp-2 text-meta leading-normal text-text-muted">
        {mark.reason}
      </p>
    </div>
  );
}

function Activity({ line }: { line: ActivityLine }) {
  return (
    <div
      className={cn(
        "flex items-center gap-6 rounded-s px-8 py-5",
        line.stalled
          ? "bg-bg inset-ring inset-ring-border-subtle"
          : "bg-primary-soft",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-7 shrink-0 rounded-full",
          line.stalled ? "bg-secondary" : "bg-primary",
        )}
      />
      <span
        className={cn(
          "min-w-0 flex-1 truncate text-meta",
          line.stalled ? "text-secondary-text" : "font-mono text-primary-text",
        )}
      >
        {line.text}
      </span>
      <span className="shrink-0 text-meta text-text-muted">{line.since}</span>
    </div>
  );
}

/**
 * One read-only card. The title is the only control: a link stretched over the card, so a keyboard user opens the
 * task with Enter and nothing on the card can be dragged or moved.
 */
export function BoardCard({
  slug,
  card,
  now,
}: {
  slug: string;
  card: BoardCardData;
  now: number;
}) {
  // Live workspace state is written only by the event stream, so this query observes the cache and never fetches.
  const { data: live } = useQuery<LiveWorkspace>({
    queryKey: keys.workspaceLive(slug, card.workspace ?? ""),
    queryFn: skipToken,
  });
  const filters = useRouterState({ select: (s) => s.location.searchStr });
  const line = activityLine(card, card.workspace ? live : undefined, now);
  const filesText = liveFilesText(line);
  const mark = attentionMark(card.attention);
  const checklist = checklistText(card);
  const blocks = blocksText(card);
  const messages = messagesText(card);
  const needs = needsText(card);
  const blocked = blockedText(card);
  const kind = kindText(card);
  const blocker = blockerMark(card);
  const leadCheck = leadCheckMark(card);
  const answerable = taskCanMarkAnswered(card.attention);
  const stalled = hasKind(card.attention, "stalled");
  const stale = hasKind(card.attention, "stale_claim");
  const hasFooter =
    card.worker !== null || stalled || stale || filesText !== null;

  return (
    <Card
      data-task={card.task}
      className="relative has-[a:focus-visible]:outline-2 has-[a:focus-visible]:outline-offset-2 has-[a:focus-visible]:outline-focus-ring"
    >
      {mark ? (
        <div className="flex flex-wrap items-center gap-6">
          <Tooltip content={mark.tooltip}>
            <span
              tabIndex={0}
              aria-label={`Needs you: ${mark.tooltip}`}
              className="relative z-10 self-start rounded-s focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring"
            >
              <Badge tone="needs-you" icon={mark.icon}>
                {mark.text}
              </Badge>
            </span>
          </Tooltip>
          {answerable ? (
            <MarkAnswered
              slug={slug}
              scope={{ task: card.task }}
              className="relative z-10"
            />
          ) : null}
        </div>
      ) : null}
      {card.labels.length > 0 ? (
        <div className="flex flex-wrap gap-4">
          {card.labels.map((label) => (
            <Label key={label.name} color={label.color}>
              {label.name}
            </Label>
          ))}
        </div>
      ) : null}
      <HrefLink
        href={taskPath(slug, card.task, filters)}
        draggable={false}
        className="text-card leading-snug font-medium text-text outline-none after:absolute after:inset-0 after:rounded-m"
      >
        {card.title}
      </HrefLink>
      {blocked ? (
        <Badge tone="blocked" className="self-start">
          {blocked}
        </Badge>
      ) : null}
      {blocker ? <BlockerPill mark={blocker} /> : null}
      <div className="flex flex-wrap items-center gap-x-10 gap-y-4 leading-snug">
        <Mono className="text-caption text-text-faint">{card.task}</Mono>
        {kind ? (
          <Meta
            icon={card.kind === "human" ? UserRound : ClipboardCheck}
            text={kind}
          />
        ) : null}
        {checklist ? <Meta icon={ListChecks} text={checklist} /> : null}
        {blocks ? <Meta icon={GitFork} text={blocks} /> : null}
        {needs ? <Meta icon={Hourglass} text={needs} /> : null}
        {messages ? (
          <Meta icon={MessageSquare} text={messages} label="messages" />
        ) : null}
      </div>
      {card.human_review ? (
        <HumanReviewBadge
          reviewer={card.human_review.reviewer}
          className="self-start"
        />
      ) : null}
      {card.proposal ? (
        <ProposalPill proposal={card.proposal} now={now} />
      ) : null}
      {leadCheck ? <LeadCheckPill mark={leadCheck} /> : null}
      {hasFooter ? (
        <div className="flex items-center justify-between gap-8 border-t border-border-subtle pt-7">
          {card.worker ? (
            <span className="min-w-0 truncate text-meta">
              <PersonName name={card.worker} />
            </span>
          ) : (
            <span />
          )}
          <span className="inline-flex shrink-0 items-center gap-4">
            {stale ? <Badge tone="stale" /> : null}
            {stalled ? <Badge tone="stalled" /> : null}
            {filesText ? (
              <span className="inline-flex items-center gap-6 rounded-s bg-primary-soft px-8 py-2 text-caption font-medium text-primary-strong">
                <span
                  aria-hidden="true"
                  className="size-7 rounded-full bg-primary"
                />
                {filesText}
              </span>
            ) : null}
          </span>
        </div>
      ) : null}
      {line ? <Activity line={line} /> : null}
    </Card>
  );
}
