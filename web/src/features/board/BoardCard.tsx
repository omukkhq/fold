import * as RadixPopover from "@radix-ui/react-popover";
import { skipToken, useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { CornerDownLeft, GitMerge, GitPullRequest } from "lucide-react";
import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import type { BoardCard as BoardCardData } from "@/api/board";
import { keys } from "@/api/keys";
import { HrefLink } from "@/app/HrefLink";
import { IdText } from "@/components/text/IdText";
import { Markdown } from "@/components/text/Markdown";
import { Chip } from "@/components/badge/Chip";
import { Card } from "@/components/card/Card";
import { Id } from "@/components/id/Id";
import { Label } from "@/components/label/Label";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import type { LiveWorkspace } from "@/live/envelope";
import {
  toldText,
  workerActivity,
  type WorkerActivity,
} from "@/live/workerActivity";
import { RestartStalled } from "@/features/workers/RestartStalled";
import { MarkAnswered } from "@/features/messages/MarkAnswered";
import { taskCanMarkAnswered } from "@/features/messages/answered";
import { activityLine, type ActivityLine } from "./activityLine";
import { currentCheckRun } from "./checkRun";
import { taskPath } from "./card";
import { cardChip, metaFact } from "./chip";
import { cardDetails, type DetailRow } from "./details";
import { hoverPeek, panelBoundary, type HoverPeek } from "./hoverPeek";

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
      {line.since === null ? null : (
        <span className="shrink-0 text-13 text-text-2">{line.since}</span>
      )}
    </p>
  );
}

function RowValue({
  slug,
  row,
  card,
  taskHref,
}: {
  slug: string;
  row: DetailRow;
  card: BoardCardData;
  taskHref: string;
}) {
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
    case "prose":
      return (
        <>
          <Markdown
            mentions={{ slug }}
            size={14}
            clamp={row.clamp ?? undefined}
            className="w-full"
          >
            {row.text}
          </Markdown>
          {row.more === null ? null : (
            <HrefLink
              href={taskHref}
              className={cn(
                "rounded-s text-13 font-semibold text-accent hover:underline",
                focusRing,
              )}
            >
              {row.more}
            </HrefLink>
          )}
        </>
      );
  }
}

/** Who fold told of the worker's stall, and Restart worker for a worker whose process fold runs. */
function StallNote({
  slug,
  activity,
}: {
  slug: string;
  activity: WorkerActivity;
}) {
  const told = toldText(activity);
  if (told === null) {
    return null;
  }
  return (
    <div className="flex flex-col gap-8">
      <p className="text-13 wrap-anywhere text-text-2">{told}</p>
      {activity.restart === null ? null : (
        <RestartStalled slug={slug} name={activity.restart} />
      )}
    </div>
  );
}

/** The facts a card leaves out, shown on hover or keyboard focus. Mark answered and, for a stalled worker, Restart worker are its buttons. */
function CardPanel({
  slug,
  card,
  line,
  activity,
  now,
  taskHref,
}: {
  slug: string;
  card: BoardCardData;
  line: ActivityLine | null;
  activity: WorkerActivity | null;
  now: number;
  taskHref: string;
}) {
  const details = cardDetails(card, line, now);
  return (
    <div className="flex flex-col gap-10">
      <p className="text-13 text-text-2">{details.head}</p>
      {line ? <Activity line={line} /> : null}
      {activity ? <StallNote slug={slug} activity={activity} /> : null}
      {details.rows.length > 0 ? (
        <dl className="grid grid-cols-[var(--width-fact-label)_minmax(0,1fr)] gap-x-10 gap-y-5">
          {details.rows.map((row) =>
            row.kind === "prose" ? (
              // A free text takes the panel's whole width under its name, so it reads as lines, not as a narrow column.
              <Fragment key={row.name}>
                <dt className="col-span-2 pt-2 text-13 text-text-2">
                  {row.name}
                </dt>
                <dd className="col-span-2 flex min-w-0 flex-col items-start gap-4">
                  <RowValue
                    slug={slug}
                    row={row}
                    card={card}
                    taskHref={taskHref}
                  />
                </dd>
              </Fragment>
            ) : (
              <Fragment key={row.name}>
                <dt className="pt-2 text-13 text-text-2">{row.name}</dt>
                <dd className="flex min-w-0 flex-wrap items-center gap-6 tabular-nums">
                  <RowValue
                    slug={slug}
                    row={row}
                    card={card}
                    taskHref={taskHref}
                  />
                </dd>
              </Fragment>
            ),
          )}
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
    parts.push(
      <PersonName
        key="worker"
        name={card.worker}
        className="min-w-0 truncate"
      />,
    );
  } else if (fact !== null) {
    parts.push(
      <span key="fact" className="min-w-0 truncate">
        {fact}
      </span>,
    );
  }
  // One line: the last part gives way and is cut short, so no part wraps below the others with its separator.
  return (
    <p className="flex min-w-0 items-center gap-x-6 overflow-hidden text-13 leading-normal whitespace-nowrap text-text-2">
      {parts.map((part, index) => (
        <span
          key={index}
          className={cn(
            "inline-flex items-center gap-6",
            index === parts.length - 1 ? "min-w-0" : "shrink-0",
          )}
        >
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
  const run = currentCheckRun(card.check_run, streamed);
  const activity =
    run === null && card.status !== "done"
      ? workerActivity(streamed, now)
      : null;
  const chip = cardChip(card, run, activity);
  const [open, peek] = usePeek();
  const box = useRef<HTMLDivElement>(null);
  const [boundary, setBoundary] = useState<Element | null>(null);
  const href = taskPath(slug, card.task, filters);
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
            if (!withPeek) return;
            setBoundary(panelBoundary(event.currentTarget));
            peek.pointerEnter(event.pointerType);
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
          onFocus={(event) => {
            if (!withPeek) return;
            setBoundary(panelBoundary(event.currentTarget));
            peek.focusIn();
          }}
          onBlur={(event) => {
            if (withPeek) peek.focusOut(inside(event.relatedTarget));
          }}
        >
          <HrefLink
            href={href}
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
              collisionBoundary={boundary}
              sticky="always"
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
              // The panel is at most as tall as the board area leaves it, and its rows scroll inside it.
              className="z-pop max-h-(--radix-popover-content-available-height) w-side-panel overflow-y-auto overscroll-contain rounded-m bg-surface px-16 py-14 text-14 text-text shadow-pop outline-none"
            >
              <CardPanel
                slug={slug}
                card={card}
                line={line}
                activity={activity}
                now={now}
                taskHref={href}
              />
            </RadixPopover.Content>
          ) : null}
        </Card>
      </RadixPopover.Anchor>
    </RadixPopover.Root>
  );
}
