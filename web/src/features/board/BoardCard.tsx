import * as RadixPopover from "@radix-ui/react-popover";
import { skipToken, useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { CornerDownLeft, GitMerge, GitPullRequest } from "lucide-react";
import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import type { BoardCard as BoardCardData } from "@/api/board";
import { keys } from "@/api/keys";
import { HrefLink } from "@/app/HrefLink";
import { IdText } from "@/components/text/IdText";
import { Chip } from "@/components/badge/Chip";
import { Card } from "@/components/card/Card";
import { Id } from "@/components/id/Id";
import { Label } from "@/components/label/Label";
import { cn } from "@/lib/cn";
import { PersonName } from "@/people/PersonName";
import type { LiveWorkspace } from "@/live/envelope";
import { MarkAnswered } from "@/features/messages/MarkAnswered";
import { taskCanMarkAnswered } from "@/features/messages/answered";
import { activityLine, type ActivityLine } from "./activityLine";
import { currentCheckRun } from "./checkRun";
import { taskPath } from "./card";
import { cardChip, metaFact } from "./chip";
import { cardDetails, type DetailRow } from "./details";
import { hoverPeek, type HoverPeek } from "./hoverPeek";

const PROPOSAL_TONE = {
  attention: "text-bad",
  active: "text-accent",
  merged: "text-text-2",
} as const;

function Activity({ line }: { line: ActivityLine }) {
  return (
    <p
      className={cn(
        "flex items-baseline gap-8 rounded-s px-10 py-8",
        line.stalled ? "bg-warn-soft" : "bg-accent-soft",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-8 shrink-0 self-center rounded-full",
          line.stalled ? "bg-warn" : "bg-accent",
        )}
      />
      <span className="min-w-0 flex-1 font-medium wrap-anywhere">
        {line.text}
      </span>
      <span className="shrink-0 text-13 text-text-2">{line.since}</span>
    </p>
  );
}

function RowValue({ row, card }: { row: DetailRow; card: BoardCardData }) {
  switch (row.kind) {
    case "person":
      return <PersonName name={row.person} />;
    case "labels":
      return (
        <>
          {card.labels.map((label) => (
            <Label key={label.name} color={label.color}>
              {label.name}
            </Label>
          ))}
        </>
      );
    case "proposal": {
      const Icon = row.look === "merged" ? GitMerge : GitPullRequest;
      return (
        <>
          <Icon
            aria-hidden="true"
            size={14}
            className={PROPOSAL_TONE[row.look]}
          />
          {row.text}
        </>
      );
    }
    case "text":
      return <span className="wrap-anywhere">{row.text}</span>;
  }
}

/** The facts a card leaves out, shown on hover or keyboard focus. Mark answered is its one button. */
function CardPanel({
  slug,
  card,
  line,
  now,
}: {
  slug: string;
  card: BoardCardData;
  line: ActivityLine | null;
  now: number;
}) {
  const details = cardDetails(card, line, now);
  return (
    <div className="flex flex-col gap-10">
      <p className="text-13 text-text-2">{details.head}</p>
      {line ? <Activity line={line} /> : null}
      {details.rows.length > 0 ? (
        <dl className="grid grid-cols-[var(--width-fact-label)_minmax(0,1fr)] gap-x-10 gap-y-5">
          {details.rows.map((row) => (
            <Fragment key={row.name}>
              <dt className="pt-2 text-13 text-text-2">{row.name}</dt>
              <dd className="flex min-w-0 flex-wrap items-center gap-6 tabular-nums">
                <RowValue row={row} card={card} />
              </dd>
            </Fragment>
          ))}
        </dl>
      ) : null}
      {details.notes.map((note) => (
        <p key={note} className="text-13 text-text-2">
          {note}
        </p>
      ))}
      {details.why ? (
        <p className="text-13 text-text-2">{details.why}</p>
      ) : null}
      {taskCanMarkAnswered(card.attention) ? (
        <MarkAnswered slug={slug} scope={{ task: card.task }} />
      ) : null}
      <p className="flex items-center gap-8 text-13 text-text-2">
        <CornerDownLeft aria-hidden="true" size={14} />
        Click or press Enter to open the task
      </p>
    </div>
  );
}

function MetaLine({ card }: { card: BoardCardData }) {
  const fact = metaFact(card);
  const parts: ReactNode[] = [
    <Id key="id">{card.task}</Id>,
    ...card.labels.map((label) => (
      <Label key={`label-${label.name}`} color={label.color}>
        {label.name}
      </Label>
    )),
  ];
  if (card.worker !== null) {
    parts.push(<PersonName key="worker" name={card.worker} />);
  } else if (fact !== null) {
    parts.push(<span key="fact">{fact}</span>);
  }
  // Each separator travels with the part after it, so a wrapped line never ends in a dot.
  return (
    <p className="flex flex-wrap items-center gap-x-6 text-13 leading-normal text-text-2">
      {parts.map((part, index) => (
        <span key={index} className="inline-flex items-center gap-6">
          {index > 0 ? <span aria-hidden="true">·</span> : null}
          {part}
        </span>
      ))}
    </p>
  );
}

function usePeek(): [boolean, HoverPeek] {
  const [open, setOpen] = useState(false);
  const [peek] = useState(() =>
    hoverPeek(setOpen, {
      set: (run, ms) => window.setTimeout(run, ms),
      clear: (id) => {
        window.clearTimeout(id);
      },
    }),
  );
  useEffect(() => peek.stop, [peek]);
  return [open, peek];
}

/**
 * One read-only card: the title, one line of facts and at most one chip. The title is the only control on the card: a
 * link stretched over it, so a keyboard user opens the task with Enter and nothing can be dragged or moved. The rest
 * shows in a panel on hover or keyboard focus; `peek` is off on a phone, which has no hover.
 */
export function BoardCard({
  slug,
  card,
  now,
  peek: withPeek = true,
}: {
  slug: string;
  card: BoardCardData;
  now: number;
  peek?: boolean;
}) {
  // Live workspace state is written only by the event stream, so this query observes the cache and never fetches.
  const { data: live } = useQuery<LiveWorkspace>({
    queryKey: keys.workspaceLive(slug, card.workspace ?? ""),
    queryFn: skipToken,
  });
  const filters = useRouterState({ select: (s) => s.location.searchStr });
  const streamed = card.workspace ? live : undefined;
  const line = activityLine(card, streamed, now);
  const chip = cardChip(card, currentCheckRun(card.check_run, streamed));
  const [open, peek] = usePeek();
  const box = useRef<HTMLDivElement>(null);
  const inside = (target: EventTarget | null) =>
    target instanceof Node && box.current?.contains(target) === true;
  const shown = withPeek && open;

  return (
    <RadixPopover.Root
      open={shown}
      onOpenChange={(next) => {
        if (!next) peek.dismiss();
      }}
    >
      <RadixPopover.Anchor asChild>
        <Card
          ref={box}
          data-task={card.task}
          className={cn(
            "relative gap-6 px-14 pt-12 pb-13 has-[a:focus-visible]:outline-2 has-[a:focus-visible]:outline-offset-2 has-[a:focus-visible]:outline-accent",
            shown && "outline-2 outline-offset-2 outline-accent",
          )}
          onPointerEnter={(event) => {
            if (withPeek) peek.pointerEnter(event.pointerType);
          }}
          onPointerLeave={() => {
            if (withPeek) peek.pointerLeave();
          }}
          onPointerDown={(event) => {
            // The panel is inside the card in the DOM, and a press there has its own rule.
            const inPanel =
              event.target instanceof Element &&
              event.target.closest("[data-peek-panel]") !== null;
            if (withPeek && !inPanel) peek.cardPress();
          }}
          onFocus={() => {
            if (withPeek) peek.focusIn();
          }}
          onBlur={(event) => {
            if (withPeek) peek.focusOut(inside(event.relatedTarget));
          }}
        >
          <HrefLink
            href={taskPath(slug, card.task, filters)}
            draggable={false}
            className="font-title text-16 leading-tight font-semibold wrap-anywhere text-text outline-none after:absolute after:inset-0 after:rounded-m"
          >
            {card.title}
          </HrefLink>
          <MetaLine card={card} />
          {chip ? (
            // A long chip, such as a list of blockers, wraps inside the card; its ids stay whole.
            <Chip
              tone={chip.tone}
              className="mt-2 max-w-full self-start whitespace-normal"
            >
              <chip.icon aria-hidden="true" size={14} className="shrink-0" />
              <span className="min-w-0">
                <IdText text={chip.text} />
              </span>
            </Chip>
          ) : null}
          {shown ? (
            // Not in a portal: the panel follows the title in the DOM, so Tab reaches Mark answered.
            <RadixPopover.Content
              side="right"
              align="start"
              sideOffset={12}
              collisionPadding={12}
              onOpenAutoFocus={(event) => {
                event.preventDefault();
              }}
              onCloseAutoFocus={(event) => {
                event.preventDefault();
              }}
              onFocusOutside={(event) => {
                if (inside(event.target)) event.preventDefault();
              }}
              onPointerDownOutside={(event) => {
                if (inside(event.target)) event.preventDefault();
              }}
              aria-label={`${card.task} details`}
              data-peek-panel=""
              onPointerDown={() => {
                peek.press();
              }}
              className="z-10 w-side-panel rounded-m bg-surface px-16 py-14 text-14 text-text shadow-pop outline-none"
            >
              <CardPanel slug={slug} card={card} line={line} now={now} />
            </RadixPopover.Content>
          ) : null}
        </Card>
      </RadixPopover.Anchor>
    </RadixPopover.Root>
  );
}
