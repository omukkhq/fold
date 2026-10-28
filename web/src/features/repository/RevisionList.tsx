import type { Revision } from "@/api/repository";
import { Chip } from "@/components/badge/Chip";
import { CommitId } from "@/components/mono/CommitId";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ActorText } from "@/people/ActorText";
import { describeRevision } from "./describe";

interface RevisionListProps {
  /** Main's revisions, newest first: the first is the head of main. */
  revisions: Revision[];
  selected: string;
  now: number;
  onSelect: (id: string) => void;
}

const DETAIL = "col-start-2 text-13 leading-normal text-text-2 tabular-nums";

/** Each revision reads as `fld log` prints it: the id and the title, then what it is and who made it, then when, with its Git commit. */
export function RevisionList({
  revisions,
  selected,
  now,
  onSelect,
}: RevisionListProps) {
  return (
    <ul
      aria-label="Main revisions"
      className="flex flex-col gap-2 rounded-m bg-surface p-6"
    >
      {revisions.map((revision, index) => {
        const lines = describeRevision(revision, now);
        const current = revision.id === selected;
        return (
          <li key={revision.id}>
            <button
              type="button"
              aria-current={current ? "true" : undefined}
              onClick={() => {
                onSelect(revision.id);
              }}
              className={cn(
                "grid w-full grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-10 rounded-s px-10 py-9 text-left",
                current ? "bg-accent-soft" : "hover:bg-fill/55",
                focusRing,
              )}
            >
              <span
                className={cn(
                  "min-w-32 font-title text-15 leading-tight font-semibold whitespace-nowrap tabular-nums",
                  current ? "text-accent" : "text-text",
                )}
              >
                {revision.id}
              </span>
              <span className="flex min-w-0 flex-wrap items-baseline gap-x-8 gap-y-2 text-15 leading-tight font-medium text-text">
                <span className="min-w-0">{lines.title}</span>
                {index === 0 ? <Chip>head of main</Chip> : null}
              </span>
              <span className={cn(DETAIL, "mt-3")}>
                ↳ <ActorText segments={lines.what} />
              </span>
              <span className={DETAIL}>
                ↳ {lines.when}
                {revision.commit === null ? null : (
                  <>
                    {" · Git "}
                    <CommitId
                      commit={revision.commit}
                      copy={false}
                      className="text-13 text-text-2"
                    />
                  </>
                )}
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
