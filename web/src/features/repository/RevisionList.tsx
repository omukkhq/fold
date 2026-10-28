import type { Revision } from "@/api/repository";
import { CommitId } from "@/components/mono/CommitId";
import { Mono } from "@/components/mono/Mono";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ActorText } from "@/people/ActorText";
import { describeRevision } from "./describe";

interface RevisionListProps {
  revisions: Revision[];
  selected: string;
  now: number;
  onSelect: (id: string) => void;
}

export function RevisionList({
  revisions,
  selected,
  now,
  onSelect,
}: RevisionListProps) {
  return (
    <ul
      aria-label="Main revisions"
      className="flex flex-col overflow-hidden rounded-l bg-surface inset-ring inset-ring-line"
    >
      {revisions.map((revision) => {
        const lines = describeRevision(revision, now);
        const current = revision.id === selected;
        return (
          <li
            key={revision.id}
            className="border-b border-line last:border-b-0"
          >
            <button
              type="button"
              aria-current={current ? "true" : undefined}
              onClick={() => {
                onSelect(revision.id);
              }}
              className={cn(
                "flex w-full items-start gap-12 px-14 py-10 text-left",
                current ? "bg-accent-soft" : "hover:bg-sunken",
                focusRing,
              )}
            >
              <Mono
                className={cn(
                  "w-32 shrink-0 font-semibold",
                  current ? "text-accent" : "text-text",
                )}
              >
                {revision.id}
              </Mono>
              <span className="flex min-w-0 flex-1 flex-col gap-3">
                <span className="truncate text-14 leading-tight font-medium text-text">
                  {lines.title}
                </span>
                <Mono className="truncate text-13 text-text-2">
                  <ActorText segments={lines.meta} />
                  {revision.commit === null ? null : " · "}
                  <CommitId
                    commit={revision.commit}
                    copy={false}
                    className="text-13"
                  />
                </Mono>
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
