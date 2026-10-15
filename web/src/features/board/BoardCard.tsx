import { skipToken, useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import {
  GitFork,
  GitMerge,
  GitPullRequest,
  Hourglass,
  ListChecks,
  type LucideIcon,
} from "lucide-react";
import type { BoardCard as BoardCardData, BoardProposal } from "@/api/board";
import { keys } from "@/api/keys";
import { HrefLink } from "@/app/HrefLink";
import { Avatar } from "@/components/avatar/Avatar";
import { Badge } from "@/components/badge/Badge";
import { Card } from "@/components/card/Card";
import { Label } from "@/components/label/Label";
import { Mono } from "@/components/mono/Mono";
import { Tooltip } from "@/components/overlay/Tooltip";
import { cn } from "@/lib/cn";
import type { LiveWorkspace } from "@/live/envelope";
import { activityLine, liveFilesText, type ActivityLine } from "./activityLine";
import { attentionMark, hasKind } from "./attention";
import { agentOf } from "./filters";
import {
  blockedText,
  blocksText,
  checklistText,
  humanReviewText,
  needsText,
  proposalLook,
  proposalText,
  taskPath,
  type ProposalLook,
} from "./card";

const PROPOSAL_ICON: Record<ProposalLook, { icon: LucideIcon; color: string }> =
  {
    attention: { icon: GitPullRequest, color: "text-danger" },
    active: { icon: GitPullRequest, color: "text-primary-text" },
    merged: { icon: GitMerge, color: "text-text-muted" },
  };

function Meta({ icon: Icon, text }: { icon: LucideIcon; text: string }) {
  return (
    <span className="inline-flex items-center gap-4 text-meta text-text-muted">
      <Icon aria-hidden="true" size={13} />
      {text}
    </span>
  );
}

function ProposalPill({ proposal }: { proposal: BoardProposal }) {
  const { icon: Icon, color } = PROPOSAL_ICON[proposalLook(proposal)];
  return (
    <span className="inline-flex items-center gap-4 self-start rounded-s bg-surface px-7 py-2 text-meta font-medium text-text inset-ring inset-ring-border">
      <Icon aria-hidden="true" size={12} className={color} />
      {proposalText(proposal)}
    </span>
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
  const needs = needsText(card);
  const blocked = blockedText(card);
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
        <Tooltip content={mark.tooltip}>
          <span
            tabIndex={0}
            aria-label={`Needs you: ${mark.tooltip}`}
            className="relative z-10 self-start rounded-s outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring"
          >
            <Badge tone="needs-you" icon={mark.icon}>
              {mark.text}
            </Badge>
          </span>
        </Tooltip>
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
      <div className="flex flex-wrap items-center gap-x-10 gap-y-4 leading-snug">
        <Mono className="text-caption text-text-faint">{card.task}</Mono>
        {checklist ? <Meta icon={ListChecks} text={checklist} /> : null}
        {blocks ? <Meta icon={GitFork} text={blocks} /> : null}
        {needs ? <Meta icon={Hourglass} text={needs} /> : null}
      </div>
      {card.human_review ? (
        <Badge tone="human-review" className="self-start">
          {humanReviewText(card.human_review)}
        </Badge>
      ) : null}
      {card.proposal ? <ProposalPill proposal={card.proposal} /> : null}
      {hasFooter ? (
        <div className="flex items-center justify-between gap-8 border-t border-border-subtle pt-7">
          {card.worker ? (
            <span className="inline-flex min-w-0 items-center gap-6">
              <Avatar name={agentOf(card.worker)} />
              <Mono className="truncate text-caption text-text-muted">
                {card.worker}
              </Mono>
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
