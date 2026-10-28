import {
  ChevronDown,
  CornerDownRight,
  History,
  ShieldAlert,
  UserPlus,
  Users,
} from "lucide-react";
import { type ReactNode, useId, useState } from "react";
import { HrefLink } from "@/app/HrefLink";
import { Button, IconButton } from "@/components/button/Button";
import { IdText } from "@/components/text/IdText";
import { ClampText } from "@/components/text/ClampText";
import { labelColors } from "@/components/label/labelColors";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { projectPath } from "@/routes/paths";
import { ErrorTitle, RowErrorText, StatePill, StepText } from "./parts";
import { RestartWorker } from "./RestartWorker";
import { SessionLine } from "./SessionLine";
import { StopWorker } from "./StopWorker";
import {
  areaNote,
  groupName,
  instructionsSubject,
  isSupervisedMain,
  mainNote,
  opensToSession,
  sessionsFact,
  type TeamGroup,
  type TeamRow,
} from "./team";

/**
 * One grid for the column heads and every row from `md`: worker, state, bound to, cost, actions.
 * Below `md` a row is a card of two columns: name and state, then bound and cost, then the step and the actions across.
 */
const GRID =
  "grid grid-cols-[minmax(0,1fr)_auto] gap-x-12 gap-y-4 md:grid-cols-[minmax(0,5fr)_minmax(0,6fr)_minmax(0,3fr)_minmax(0,2fr)_minmax(0,3fr)] md:gap-x-18";

const CELL = {
  who: "col-start-1 row-start-1 md:row-span-2 md:self-center",
  chip: "col-start-2 row-start-1 justify-self-end md:justify-self-start md:self-end",
  step: "col-span-2 row-start-3 md:col-span-1 md:col-start-2 md:row-start-2 md:self-start",
  bound:
    "col-start-1 row-start-2 md:col-start-3 md:row-span-2 md:row-start-1 md:self-center",
  cost: "col-start-2 row-start-2 text-right md:col-start-4 md:row-span-2 md:row-start-1 md:self-center",
  acts: "col-span-2 row-start-4 md:col-span-1 md:col-start-5 md:row-span-2 md:row-start-1 md:self-center md:justify-self-end",
} as const;

const KEY = "font-title text-13 leading-tight font-semibold text-text-2";

function BoundCell({ slug, row }: { slug: string; row: TeamRow }) {
  const { bound } = row;
  if (bound.kind === "person") {
    return <PersonName name={bound.name} className="text-14" />;
  }
  if (bound.kind === "text") {
    return <span className="text-14 text-text-2">{bound.text}</span>;
  }
  const text = (
    <span className="text-14 text-text">
      <IdText text={bound.text} />
    </span>
  );
  return bound.batch === null ? (
    text
  ) : (
    <HrefLink
      href={projectPath(slug, "batches", bound.batch)}
      className={cn("rounded-s underline-offset-2 hover:underline", focusRing)}
    >
      {text}
    </HrefLink>
  );
}

/** The worker's session still holds the skill of an older fold, until its next turn tells it to load the skill again. */
function OlderFoldMark() {
  return (
    <span className="flex shrink-0 items-center gap-4 text-13 whitespace-nowrap text-text-2">
      <History aria-hidden="true" size={14} />
      older fold
    </span>
  );
}

/** The permission mark: the worker waits for a person to answer what Claude Code asks. */
function AsksMark() {
  return (
    <span className="flex shrink-0 items-center gap-4 text-13 font-medium whitespace-nowrap text-warn">
      <ShieldAlert aria-hidden="true" size={14} />
      asks you
    </span>
  );
}

function Row({
  slug,
  row,
  human,
}: {
  slug: string;
  row: TeamRow;
  human: string | null;
}) {
  const detailId = useId();
  const [open, setOpen] = useState(false);
  // Only a named human stops a worker or starts it in a new session.
  const stop = human === null ? null : row.stop;
  return (
    <li className="flex flex-col">
      <div
        className={cn(
          GRID,
          "px-16 py-10 hover:bg-fill/45 md:min-h-bar-compact md:py-8",
        )}
      >
        <div className={cn(CELL.who, "flex min-w-0 items-center gap-8")}>
          {row.child ? (
            <CornerDownRight
              aria-hidden="true"
              size={14}
              className="shrink-0 text-text-2 md:ml-4"
            />
          ) : null}
          <div className="flex min-w-0 flex-col gap-2 leading-tight">
            <PersonName name={row.name} className="truncate text-15" />
            <span className="text-13 text-text-2">{row.roleLine}</span>
          </div>
        </div>
        <div
          className={cn(
            CELL.chip,
            "flex flex-wrap items-center gap-x-8 gap-y-4",
          )}
        >
          <StatePill pill={row.pill} />
          {row.asks ? <AsksMark /> : null}
          {row.olderFold ? <OlderFoldMark /> : null}
        </div>
        <div className={cn(CELL.step, "min-w-0")}>
          {row.errorInStep && row.error !== null ? (
            <ErrorTitle error={row.error} />
          ) : (
            <StepText step={row.step} warn={row.asks} />
          )}
        </div>
        <div className={cn(CELL.bound, "min-w-0")}>
          <BoundCell slug={slug} row={row} />
        </div>
        <div className={cn(CELL.cost, "text-14 text-text tabular-nums")}>
          {row.cost}
        </div>
        <div className={cn(CELL.acts, "flex items-start gap-6")}>
          {stop === null ? null : (
            <StopWorker
              slug={slug}
              name={row.name}
              control={stop}
              size="sm"
              idleLabel="Stop"
              busyLabel="Stopping…"
              ariaLabel={`Stop ${row.name}`}
              ariaBusyLabel={`Stopping ${row.name}…`}
              className="items-start md:items-end"
              note={row.stopNote}
            />
          )}
          {opensToSession(row) ? (
            <IconButton
              size="sm"
              variant="ghost"
              aria-label={`Session of @${row.name}`}
              aria-expanded={open}
              aria-controls={detailId}
              onClick={() => {
                setOpen(!open);
              }}
            >
              <ChevronDown
                aria-hidden="true"
                size={16}
                className={cn(
                  "transition-transform motion-reduce:transition-none",
                  open && "rotate-180",
                )}
              />
            </IconButton>
          ) : null}
        </div>
      </div>
      {open ? (
        <Detail id={detailId} slug={slug} row={row} human={human} />
      ) : null}
    </li>
  );
}

/** The open row: what the worker runs on, its last error, its session and transcript to copy, and New session. */
function Detail({
  id,
  slug,
  row,
  human,
}: {
  id: string;
  slug: string;
  row: TeamRow;
  human: string | null;
}) {
  const facts: [string, string][] = [
    ["Model", row.model],
    ["Turns", row.turns],
    ["Context", row.context],
    ["Sessions", sessionsFact(row.previousSessions)],
  ];
  return (
    <div
      id={id}
      className="mx-16 mt-2 mb-10 grid grid-cols-1 gap-x-28 gap-y-14 rounded-m bg-fill px-16 py-14 lg:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]"
    >
      <dl className="grid grid-cols-2 content-start gap-x-28 gap-y-8 sm:grid-cols-4 lg:grid-cols-2 xl:grid-cols-4">
        {facts.map(([key, value]) => (
          <div key={key} className="flex min-w-0 flex-col gap-2">
            <dt className={KEY}>{key}</dt>
            <dd className="text-14 text-text tabular-nums">{value}</dd>
          </div>
        ))}
      </dl>
      <div className="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-14 gap-y-6">
        {row.error === null ? null : (
          <>
            <span className={KEY}>Error</span>
            <RowErrorText error={row.error} />
          </>
        )}
        {row.session === null ? null : <SessionLine session={row.session} />}
      </div>
      {human !== null && row.restart ? (
        <div className="flex flex-wrap items-center gap-12 lg:col-span-2">
          <RestartWorker
            slug={slug}
            name={row.name}
            previousSessions={row.previousSessions}
            className="bg-surface"
          />
          <span className="max-w-measure text-13 text-text-2">
            Stops the worker and starts it again with a new Claude Code session.
            It keeps its name, its claims and its workers.
          </span>
        </div>
      ) : null}
    </div>
  );
}

function GroupHead({
  group,
  human,
}: {
  group: TeamGroup;
  human: string | null;
}) {
  let note: ReactNode;
  if (group.kind === "main") {
    note = (
      <>
        {mainNote(group)} ·{" "}
        {human === null || isSupervisedMain(group) ? null : (
          <>
            <PersonName name={`human:${human}`} /> ·{" "}
          </>
        )}
        talks, plans tasks, starts the leads
      </>
    );
  } else if (group.kind === "review") {
    note = "review lead · gives open proposals to reviewers";
  } else {
    note = [areaNote(group), ...group.profile.facts].join(" · ");
  }
  return (
    <div className="flex flex-col gap-4 px-16 pt-12 pb-6">
      <div className="flex flex-wrap items-baseline gap-x-12 gap-y-4">
        <h2 className="flex items-center gap-8 text-16 font-semibold text-text">
          {group.label === null ? null : (
            <span
              aria-hidden="true"
              className={cn(
                "size-9 shrink-0 rounded-full",
                labelColors(group.label.color).dot,
              )}
            />
          )}
          {groupName(group)}
        </h2>
        <span className="text-13 text-text-2">{note}</span>
      </div>
      {group.profile.instructions === null ? null : (
        <ClampText
          lines={2}
          subject={instructionsSubject(group)}
          className="max-w-measure text-14 text-text"
        >
          {group.profile.instructions}
        </ClampText>
      )}
    </div>
  );
}

export function TeamTable({
  slug,
  groups,
  human,
  noLeads,
  mainName,
  onStartLead,
}: {
  slug: string;
  groups: readonly TeamGroup[];
  human: string | null;
  /** Only the main agents so far: the list ends with the no-leads state. */
  noLeads: boolean;
  mainName: string;
  /** Null until the human has a name: only a named human starts a lead. */
  onStartLead: (() => void) | null;
}) {
  return (
    <section aria-label="Team" className="flex min-w-0 flex-1 flex-col gap-14">
      <div
        aria-hidden="true"
        className={cn(
          GRID,
          "hidden px-16 font-title text-13 font-semibold text-text-2 md:grid",
        )}
      >
        <span>Worker</span>
        <span>State</span>
        <span>Bound to</span>
        <span className="text-right">Cost</span>
        <span />
      </div>
      {groups.map((group) =>
        noLeads && group.kind !== "main" ? null : (
          <section
            key={group.key}
            aria-label={groupName(group)}
            className="flex flex-col rounded-m bg-surface pt-4 pb-8"
          >
            <GroupHead group={group} human={human} />
            <ul aria-label={`Workers of ${groupName(group)}`}>
              {group.rows.map((row) => (
                <Row key={row.name} slug={slug} row={row} human={human} />
              ))}
            </ul>
          </section>
        ),
      )}
      {noLeads ? (
        <section
          aria-label="No leads yet"
          className="flex flex-col items-center gap-10 rounded-m bg-surface px-24 py-40 text-center"
        >
          <Users aria-hidden="true" size={28} className="text-text-2" />
          <h2 className="text-18 leading-tight font-semibold text-text">
            No leads yet
          </h2>
          <p className="max-w-measure text-15 text-text-2">
            Ask <PersonName name={mainName} /> to plan the work, or start a lead
            for an area or for review.
          </p>
          {onStartLead === null ? null : (
            <Button
              variant="primary"
              icon={<UserPlus aria-hidden="true" size={16} />}
              onClick={onStartLead}
            >
              Start a lead
            </Button>
          )}
        </section>
      ) : null}
    </section>
  );
}
