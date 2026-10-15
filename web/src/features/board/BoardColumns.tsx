import { Plus } from "lucide-react";
import type { ReactNode } from "react";
import { HrefLink } from "@/app/HrefLink";
import { buttonClasses } from "@/components/button/buttonStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { Skeleton } from "@/components/states/Skeleton";
import { Mono } from "@/components/mono/Mono";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { BoardCard } from "./BoardCard";
import { newTaskPath } from "./card";
import { COLUMNS, type ColumnSpec, type ColumnView } from "./columns";

const LOADING_CARDS = [0, 1, 2];

function Column({
  spec,
  count,
  children,
}: {
  spec: ColumnSpec;
  /** The count beside the name; `–` while the board loads. */
  count: string;
  children?: ReactNode;
}) {
  return (
    <section
      aria-label={spec.name}
      data-status={spec.status}
      className="flex min-w-0 flex-1 flex-col gap-8"
    >
      <header className="flex flex-col">
        <div className="flex items-center gap-8 p-4">
          <span
            aria-hidden="true"
            className={cn("size-8 shrink-0 rounded-full", spec.dot)}
          />
          <h2 className="text-card leading-snug font-semibold text-text">
            {spec.name}
          </h2>
          <span className="text-body font-medium text-text-faint">{count}</span>
        </div>
        <p className="text-meta leading-snug text-text-faint">{spec.hint}</p>
      </header>
      <div className="flex flex-col gap-10 pt-2">{children}</div>
    </section>
  );
}

function Row({ children }: { children: ReactNode }) {
  return <div className="flex flex-1 gap-16 px-24 pt-10 pb-24">{children}</div>;
}

export function BoardColumns({
  slug,
  columns,
  now,
}: {
  slug: string;
  columns: readonly ColumnView[];
  now: number;
}) {
  return (
    <Row>
      {columns.map((column) => (
        <Column
          key={column.status}
          spec={column}
          count={String(column.cards.length)}
        >
          {column.cards.length > 0 ? (
            <ol className="flex flex-col gap-10">
              {column.cards.map((card) => (
                <li key={card.task}>
                  <BoardCard slug={slug} card={card} now={now} />
                </li>
              ))}
            </ol>
          ) : null}
        </Column>
      ))}
    </Row>
  );
}

/** W02 · loading: every column with three skeleton cards and no count. */
export function LoadingColumns() {
  return (
    <Row>
      {COLUMNS.map((spec) => (
        <Column key={spec.status} spec={spec} count="–">
          {LOADING_CARDS.map((i) => (
            <Skeleton key={i} />
          ))}
        </Column>
      ))}
    </Row>
  );
}

/** W02 · empty: the five columns at zero, and the next step in the Todo column, where new tasks arrive. */
export function EmptyColumns({ slug }: { slug: string }) {
  return (
    <Row>
      {COLUMNS.map((spec) => (
        <Column key={spec.status} spec={spec} count="0">
          {spec.status === "todo" ? (
            <EmptyPanel
              title="No tasks yet"
              action={
                <>
                  <Mono className="block whitespace-pre-wrap text-meta text-text">
                    {"FOLD_AGENT=frontend \\\n  claude --name frontend"}
                  </Mono>
                  <HrefLink
                    href={newTaskPath(slug)}
                    className={cn(
                      buttonClasses("secondary", "md"),
                      focusRing,
                      "self-start",
                    )}
                  >
                    <Plus
                      aria-hidden="true"
                      size={15}
                      className="text-text-muted"
                    />
                    New task
                  </HrefLink>
                </>
              }
            >
              Give an orchestrator a goal; it creates the tasks here. You can
              also add one yourself.
            </EmptyPanel>
          ) : null}
        </Column>
      ))}
    </Row>
  );
}

/** Column heads without cards, under the error banner. */
export function BareColumns() {
  return (
    <Row>
      {COLUMNS.map((spec) => (
        <Column key={spec.status} spec={spec} count="–" />
      ))}
    </Row>
  );
}
