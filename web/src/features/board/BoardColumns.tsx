import * as RadixTabs from "@radix-ui/react-tabs";
import { Plus } from "lucide-react";
import type { ReactNode } from "react";
import type { TaskStatus } from "@/api/board";
import { HrefLink } from "@/app/HrefLink";
import { buttonClasses } from "@/components/button/buttonStyles";
import { Mono } from "@/components/mono/Mono";
import { Tooltip } from "@/components/overlay/Tooltip";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { Skeleton } from "@/components/states/Skeleton";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { projectPath } from "@/routes/paths";
import { BoardCard } from "./BoardCard";
import { newTaskPath } from "./card";
import {
  COLUMNS,
  hintText,
  moreText,
  type ColumnSpec,
  type ColumnView,
} from "./columns";
import { phoneHint, taskCountText } from "./view";

const LOADING_CARDS = [0, 1, 2];

/** One column as either layout draws it. */
interface ColumnFrame {
  spec: ColumnSpec;
  /** The count beside the name; `–` while the board loads. */
  count: string;
  /** The column's hint: a tooltip on its head, or the line under the phone's switcher. */
  hint: string;
  /** `4 tasks` beside the phone's line; empty while the board loads. */
  tasks: string;
  body: ReactNode;
}

/** The phone shows one column at a time under a switcher; without it the five columns stand side by side. */
export interface PhoneColumns {
  selected: TaskStatus;
  onSelect: (status: TaskStatus) => void;
}

function Column({ frame }: { frame: ColumnFrame }) {
  return (
    <section
      aria-label={frame.spec.name}
      data-status={frame.spec.status}
      className="flex min-h-0 min-w-(--width-column) flex-1 flex-col rounded-l bg-sunken"
    >
      <Tooltip content={frame.hint}>
        <div
          tabIndex={0}
          className={cn(
            "flex shrink-0 items-baseline gap-8 rounded-t-l px-16 pt-12 pb-6",
            focusRing,
          )}
        >
          <h2 className="font-title text-16 leading-tight font-semibold text-text">
            {frame.spec.name}
          </h2>
          <span className="font-title text-15 font-medium text-text-2 tabular-nums">
            {frame.count}
          </span>
        </div>
      </Tooltip>
      <div className="flex min-h-0 flex-1 flex-col gap-10 overflow-y-auto px-10 pt-4 pb-12">
        {frame.body}
      </div>
    </section>
  );
}

function PhoneBoard({
  frames,
  phone,
}: {
  frames: readonly ColumnFrame[];
  phone: PhoneColumns;
}) {
  const current =
    frames.find((frame) => frame.spec.status === phone.selected) ?? frames[0];
  return (
    <RadixTabs.Root
      value={current?.spec.status}
      onValueChange={(value) => {
        const chosen = frames.find((frame) => frame.spec.status === value);
        if (chosen) phone.onSelect(chosen.spec.status);
      }}
      className="flex min-h-0 flex-1 flex-col gap-10 px-12 pt-10"
    >
      <RadixTabs.List
        aria-label="Column"
        className={cn(SEGMENT_LIST, "flex-nowrap bg-surface")}
      >
        {frames.map((frame) => (
          <RadixTabs.Trigger
            key={frame.spec.status}
            value={frame.spec.status}
            className={cn(
              segmentClasses(frame === current),
              "flex-auto px-4 whitespace-nowrap shadow-none data-[state=active]:bg-fill",
              focusRing,
            )}
          >
            {frame.spec.name}{" "}
            <span className="tabular-nums">{frame.count}</span>
          </RadixTabs.Trigger>
        ))}
      </RadixTabs.List>
      {current ? (
        <RadixTabs.Content
          value={current.spec.status}
          className={cn(
            "flex min-h-0 flex-1 flex-col gap-10 rounded-s",
            focusRing,
          )}
        >
          <section
            aria-label={current.spec.name}
            data-status={current.spec.status}
            className="flex min-h-0 flex-1 flex-col gap-10"
          >
            <p className="flex justify-between gap-8 px-4 text-13 text-text-2">
              <span>{phoneHint(current.spec.name, current.hint)}</span>
              <span className="shrink-0 tabular-nums">{current.tasks}</span>
            </p>
            <div className="flex min-h-0 flex-1 flex-col gap-10 overflow-y-auto pb-16">
              {current.body}
            </div>
          </section>
        </RadixTabs.Content>
      ) : null}
    </RadixTabs.Root>
  );
}

function Layout({
  frames,
  phone,
}: {
  frames: readonly ColumnFrame[];
  phone: PhoneColumns | null;
}) {
  if (phone !== null) {
    return <PhoneBoard frames={frames} phone={phone} />;
  }
  return (
    <div className="flex min-h-0 flex-1 gap-14 overflow-x-auto px-22 pt-10 pb-22">
      {frames.map((frame) => (
        <Column key={frame.spec.status} frame={frame} />
      ))}
    </div>
  );
}

function frame(
  spec: ColumnSpec,
  count: number | null,
  body: ReactNode,
  hint = spec.hint,
): ColumnFrame {
  return {
    spec,
    count: count === null ? "–" : String(count),
    hint,
    tasks: count === null ? "" : taskCountText(count),
    body,
  };
}

export function BoardColumns({
  slug,
  columns,
  now,
  phone,
}: {
  slug: string;
  columns: readonly ColumnView[];
  now: number;
  phone: PhoneColumns | null;
}) {
  return (
    <Layout
      phone={phone}
      frames={columns.map((column) =>
        frame(
          column,
          column.count,
          <>
            {column.cards.length > 0 ? (
              <ol className="flex flex-col gap-10">
                {column.cards.map((card) => (
                  <li key={card.task}>
                    <BoardCard
                      slug={slug}
                      card={card}
                      now={now}
                      peek={phone === null}
                    />
                  </li>
                ))}
              </ol>
            ) : null}
            <MoreLink slug={slug} text={moreText(column)} />
          </>,
          hintText(column),
        ),
      )}
    />
  );
}

function MoreLink({ slug, text }: { slug: string; text: string | null }) {
  if (text === null) {
    return null;
  }
  return (
    <HrefLink
      href={projectPath(slug, "repository")}
      className={cn(
        "self-start rounded-s px-6 text-13 font-medium text-accent hover:underline",
        focusRing,
      )}
    >
      {text}
    </HrefLink>
  );
}

/** Loading: every column with three skeleton cards and no count. */
export function LoadingColumns({ phone }: { phone: PhoneColumns | null }) {
  return (
    <Layout
      phone={phone}
      frames={COLUMNS.map((spec) =>
        frame(
          spec,
          null,
          LOADING_CARDS.map((i) => (
            <Skeleton key={i} className="inset-ring-0" />
          )),
        ),
      )}
    />
  );
}

/** Empty: the five columns at zero, and the next step in the To do column, where new tasks arrive. */
export function EmptyColumns({
  slug,
  phone,
}: {
  slug: string;
  phone: PhoneColumns | null;
}) {
  return (
    <Layout
      phone={phone}
      frames={COLUMNS.map((spec) =>
        frame(
          spec,
          0,
          spec.status === "todo" ? (
            <EmptyPanel
              title="No tasks yet"
              className="p-16"
              action={
                <>
                  <Mono className="block text-13 text-text">fld claude</Mono>
                  <HrefLink
                    href={newTaskPath(slug)}
                    className={cn(
                      buttonClasses("quiet", "md"),
                      focusRing,
                      "self-start",
                    )}
                  >
                    <Plus
                      aria-hidden="true"
                      size={16}
                      className="text-text-2"
                    />
                    New task
                  </HrefLink>
                </>
              }
            >
              Run the command below, then give the main agent a goal. It creates
              the tasks here. You can also add one yourself.
            </EmptyPanel>
          ) : null,
        ),
      )}
    />
  );
}

/** Column heads without cards, under the error banner. */
export function BareColumns({ phone }: { phone: PhoneColumns | null }) {
  return (
    <Layout
      phone={phone}
      frames={COLUMNS.map((spec) => frame(spec, null, null))}
    />
  );
}
