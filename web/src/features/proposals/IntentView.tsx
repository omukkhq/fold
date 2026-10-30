import { Check } from "lucide-react";
import type { ComparedFile } from "@/api/diff";
import type { Intent } from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionText } from "@/mentions/MentionText";
import { PART_COVERS, intentSections, partSize } from "./intent";
import { detailHref } from "./view";

const SECTION = "font-title text-14 leading-tight font-semibold text-text-2";

/** A list of one-line entries, such as the decisions. */
function Entries({
  slug,
  title,
  entries,
}: {
  slug: string;
  title: string;
  entries: readonly string[];
}) {
  return (
    <section aria-label={title} className="flex flex-col gap-8">
      <h3 className={SECTION}>{title}</h3>
      <ul className="flex max-w-measure list-disc flex-col gap-6 pl-18 text-15 leading-normal">
        {entries.map((entry, index) => (
          // Entries may repeat, so their place keeps them apart.
          <li key={index}>
            <MentionText text={entry} slug={slug} className="inline" />
          </li>
        ))}
      </ul>
    </section>
  );
}

export interface IntentBodyProps {
  slug: string;
  /** The proposal, such as P-3, whose Changes tab each file opens. */
  id: string;
  /** The version whose intent this is, whose Changes tab each file opens. */
  version: number;
  intent: Intent;
  /** The changed files of that version with their line counts, for the size of each part. */
  files: readonly ComparedFile[];
}

/** What each part of the diff does with its files, then the contracts, decisions and what the change does not change, each only when the intent has it. */
export function IntentBody({
  slug,
  id,
  version,
  intent,
  files,
}: IntentBodyProps) {
  const sections = intentSections(intent);
  return (
    <div className="flex min-w-0 flex-col gap-16">
      <section aria-label="Parts" className="flex flex-col gap-4">
        <h3 className={SECTION}>What each part of the diff does</h3>
        <ul className="flex flex-col">
          {intent.parts.map((part, index) => (
            <li
              key={part.name}
              className={cn(
                "grid grid-cols-[var(--space-18)_minmax(0,1fr)] gap-x-10 py-12",
                index > 0 && "border-t border-line",
              )}
            >
              <span
                aria-hidden="true"
                className="mt-4 grid size-14 place-items-center rounded-pill bg-ok text-surface"
              >
                <Check size={10} strokeWidth={3} />
              </span>
              <div className="flex min-w-0 flex-col gap-2">
                <span className="text-15 font-semibold text-text">
                  {part.name}
                  <span className="sr-only">{PART_COVERS}</span>
                </span>
                <MentionText
                  text={part.does}
                  slug={slug}
                  className="max-w-measure"
                />
                <div className="mt-6 flex flex-wrap items-center gap-6">
                  {part.files.map((path) => (
                    <HrefLink
                      key={path}
                      href={detailHref(slug, id, {
                        tab: "changes",
                        version,
                        path,
                      })}
                      title={path}
                      className={cn(
                        "max-w-full truncate rounded-pill bg-accent-soft px-8 py-2 font-mono text-13 text-accent hover:underline",
                        focusRing,
                      )}
                    >
                      {path}
                    </HrefLink>
                  ))}
                  <span className="text-13 text-text-2 tabular-nums">
                    {partSize(part, files)}
                  </span>
                </div>
              </div>
            </li>
          ))}
        </ul>
      </section>
      {sections.contracts.length === 0 ? null : (
        <section aria-label="Contracts" className="flex flex-col gap-8">
          <h3 className={SECTION}>Contracts it adds or changes</h3>
          <dl className="grid grid-cols-1 gap-x-14 gap-y-8 text-14 sm:grid-cols-[var(--width-fact-label)_minmax(0,1fr)]">
            {sections.contracts.map((line) => (
              <div key={line.kind} className="contents">
                <dt className="pt-2 font-title text-13 font-semibold text-text-2">
                  {line.label}
                </dt>
                <dd className="min-w-0">
                  <ul className="flex flex-col gap-2">
                    {line.entries.map((entry, index) => (
                      <li key={index}>
                        <MentionText
                          text={entry}
                          slug={slug}
                          className="text-14"
                        />
                      </li>
                    ))}
                  </ul>
                </dd>
              </div>
            ))}
          </dl>
        </section>
      )}
      {sections.decisions.length === 0 ? null : (
        <Entries slug={slug} title="Decisions" entries={sections.decisions} />
      )}
      {sections.notChanged.length === 0 ? null : (
        <Entries
          slug={slug}
          title="What it does not change"
          entries={sections.notChanged}
        />
      )}
    </div>
  );
}
