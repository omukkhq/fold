import { CornerDownRight, ShieldAlert, UserPlus, Users } from "lucide-react";
import { Fragment, type ReactNode } from "react";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { Label } from "@/components/label/Label";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { projectPath } from "@/routes/paths";
import { RowErrorText, StatePill, StepText } from "./parts";
import { RestartWorker } from "./RestartWorker";
import { SessionLine } from "./SessionLine";
import { StopWorker } from "./StopWorker";
import {
  areaNote,
  isSupervisedMain,
  mainNote,
  type TeamGroup,
  type TeamRow,
} from "./team";

const HEADS = [
  "Worker",
  "State",
  "Bound to",
  "Model",
  "Turns",
  "Cost",
  "Context",
  "Current step",
] as const;

const COLUMNS = HEADS.length + 1;
const HEAD =
  "px-6 py-9 text-left text-label leading-snug font-semibold tracking-overline whitespace-nowrap text-text-muted uppercase first:pl-16 last:pr-16";
const CELL = "px-6 py-10 align-middle first:pl-16 last:pr-16";
const NUMBER = "text-code whitespace-nowrap text-text-muted";

function BoundCell({ slug, row }: { slug: string; row: TeamRow }) {
  const { bound } = row;
  if (bound.kind === "person") {
    return <PersonName name={bound.name} slot="teamRow" />;
  }
  if (bound.kind === "text") {
    return (
      <span className="text-code whitespace-nowrap text-text-muted">
        {bound.text}
      </span>
    );
  }
  const text = (
    <span className="font-mono text-label whitespace-nowrap text-text">
      {bound.text}
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

function StopCell({
  slug,
  row,
  human,
}: {
  slug: string;
  row: TeamRow;
  human: string | null;
}) {
  // Only a named human stops a worker or starts it in a new session.
  if (human === null || (row.stop === null && !row.restart)) return null;
  return (
    <div className="flex items-start gap-6">
      {row.stop === null ? null : (
        <StopWorker
          slug={slug}
          name={row.name}
          control={row.stop}
          size="sm"
          idleLabel="Stop"
          busyLabel="Stopping…"
          ariaLabel={`Stop ${row.name}`}
          ariaBusyLabel={`Stopping ${row.name}…`}
          className="items-start"
        />
      )}
      {row.restart ? (
        <RestartWorker
          slug={slug}
          name={row.name}
          previousSessions={row.previousSessions}
        />
      ) : null}
    </div>
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
  return (
    <tr className="border-b border-border-subtle last:border-b-0">
      <td className={cn(CELL, "w-full")}>
        <div className={cn("flex items-center gap-8", row.child && "pl-22")}>
          {row.child ? (
            <CornerDownRight
              aria-hidden="true"
              size={13}
              className="shrink-0 text-text-faint"
            />
          ) : null}
          <div className="flex min-w-0 flex-col gap-2">
            <PersonName
              name={row.name}
              slot="teamRow"
              className="leading-snug"
            />
            <span className="text-meta text-text-muted">{row.roleLine}</span>
            {row.session === null ? null : (
              <SessionLine session={row.session} />
            )}
          </div>
        </div>
      </td>
      <td className={CELL}>
        <span className="flex items-center gap-6">
          <StatePill pill={row.pill} />
          {row.asks ? (
            <ShieldAlert
              role="img"
              aria-label="waits for a permission answer"
              size={14}
              className="shrink-0 text-warning"
            />
          ) : null}
        </span>
      </td>
      <td className={CELL}>
        <BoundCell slug={slug} row={row} />
      </td>
      <td className={cn(CELL, NUMBER)}>{row.model}</td>
      <td className={cn(CELL, NUMBER)}>{row.turns}</td>
      <td className={cn(CELL, NUMBER)}>{row.cost}</td>
      <td className={cn(CELL, NUMBER)}>{row.context}</td>
      <td className={CELL}>
        <StepText step={row.step} />
        {row.error === null ? null : <RowErrorText error={row.error} />}
      </td>
      <td className={CELL}>
        <StopCell slug={slug} row={row} human={human} />
      </td>
    </tr>
  );
}

function GroupHead({
  group,
  human,
}: {
  group: TeamGroup;
  human: string | null;
}) {
  let content: ReactNode;
  if (group.kind === "main") {
    content = (
      <>
        <span className="font-semibold text-text">Main agent</span>
        <span className="text-text-muted">
          {mainNote(group)} ·{" "}
          {human === null || isSupervisedMain(group) ? null : (
            <>
              <PersonName name={`human:${human}`} slot="teamNote" /> ·{" "}
            </>
          )}
          talks, plans tasks, starts the leads
        </span>
      </>
    );
  } else if (group.kind === "review") {
    content = (
      <>
        <span className="font-semibold text-text">Review</span>
        <span className="text-text-muted">
          review lead · groups open proposals into batches
        </span>
      </>
    );
  } else {
    content = (
      <>
        {group.label ? (
          <Label color={group.label.color}>{group.label.name}</Label>
        ) : null}
        <span className="text-text-muted">{areaNote(group)}</span>
        {group.profile.facts.map((fact) => (
          <span key={fact} className="whitespace-nowrap text-text-muted">
            {fact}
          </span>
        ))}
        {group.profile.instructions === null ? null : (
          <span
            className="min-w-0 truncate text-text-faint"
            title={group.profile.instructions}
          >
            {group.profile.instructions}
          </span>
        )}
      </>
    );
  }
  return (
    <tr className="border-b border-border-subtle bg-bg">
      <th
        colSpan={COLUMNS}
        scope="colgroup"
        className="px-16 py-7 text-left font-normal"
      >
        <span className="flex min-w-0 items-center gap-12 text-code">
          {content}
        </span>
      </th>
    </tr>
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
  /** Only the main agents so far: the table ends with the no-leads state. */
  noLeads: boolean;
  mainName: string;
  /** Null until the human has a name: only a named human starts a lead. */
  onStartLead: (() => void) | null;
}) {
  return (
    <div className="min-w-0 flex-1 overflow-x-auto rounded-l border border-border-subtle bg-surface">
      <table className="w-full border-collapse" aria-label="Team">
        <thead className="bg-bg">
          <tr className="border-b border-border-subtle">
            {HEADS.map((head) => (
              <th key={head} scope="col" className={HEAD}>
                {head}
              </th>
            ))}
            <th scope="col" className={HEAD}>
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {groups.map((group) =>
            noLeads && group.kind !== "main" ? null : (
              <Fragment key={group.key}>
                <GroupHead group={group} human={human} />
                {group.rows.map((row) => (
                  <Row key={row.name} slug={slug} row={row} human={human} />
                ))}
              </Fragment>
            ),
          )}
          {noLeads ? (
            <tr>
              <td colSpan={COLUMNS} className="px-24 py-40">
                <div className="flex flex-col items-center gap-10 text-center">
                  <Users
                    aria-hidden="true"
                    size={28}
                    className="text-text-faint"
                  />
                  <h2 className="text-item-title leading-snug font-semibold text-text">
                    No leads yet
                  </h2>
                  <p className="text-body text-text-muted">
                    Ask <PersonName name={mainName} slot="teamNote" /> to plan
                    the work, or start a lead for an area or for review.
                  </p>
                  {onStartLead === null ? null : (
                    <Button
                      variant="primary"
                      icon={<UserPlus aria-hidden="true" size={15} />}
                      onClick={onStartLead}
                    >
                      Start a lead
                    </Button>
                  )}
                </div>
              </td>
            </tr>
          ) : null}
        </tbody>
      </table>
    </div>
  );
}
