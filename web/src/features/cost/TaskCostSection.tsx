import { useQuery } from "@tanstack/react-query";
import { taskCostQuery } from "@/api/cost";
import { HrefLink } from "@/app/HrefLink";
import { Mono } from "@/components/mono/Mono";
import { SideGroup } from "@/features/task/parts";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { projectPath } from "@/routes/paths";
import { taskCostSection } from "./cost";

/** The task's cost in the side column: the total, the build and review parts and one row per worker. Live: the end of a worker's turn reads it again. */
export function TaskCostSection({
  slug,
  task,
}: {
  slug: string;
  task: string;
}) {
  const query = useQuery(taskCostQuery(slug, task));
  const section = query.data ? taskCostSection(query.data) : null;
  return (
    <SideGroup
      title="Cost"
      stacked
      action={
        section ? (
          <span className="text-15 font-semibold text-text tabular-nums">
            {section.total}
          </span>
        ) : undefined
      }
    >
      {query.isError ? (
        <p className="text-14 text-text-2">
          fold could not read the task's cost.
        </p>
      ) : section === null ? (
        <p className="text-14 text-text-2">Loading the cost…</p>
      ) : section.rows.length === 0 ? (
        <p className="text-14 text-text-2">
          No worker that fold runs has worked on this task yet.
        </p>
      ) : (
        <div className="flex flex-col gap-10">
          <p className="text-13 text-text-2 tabular-nums">{section.tokens}</p>
          <div className="flex flex-col gap-6">
            <div
              aria-hidden="true"
              className="flex h-6 gap-2 overflow-hidden rounded-full bg-sunken"
            >
              {section.buildPart > 0 ? (
                <span
                  className="h-full bg-accent"
                  style={{ width: `${String(section.buildPart * 100)}%` }}
                />
              ) : null}
              {section.buildPart < 1 ? (
                <span className="h-full flex-1 bg-warn" />
              ) : null}
            </div>
            <p className="flex flex-wrap gap-x-12 gap-y-2 text-13 text-text-2">
              <span className="flex items-center gap-6">
                <span
                  aria-hidden="true"
                  className="size-7 rounded-full bg-accent"
                />
                Build {section.build}
              </span>
              <span className="flex items-center gap-6">
                <span
                  aria-hidden="true"
                  className="size-7 rounded-full bg-warn"
                />
                Review {section.review}
              </span>
            </p>
          </div>
          <ul className="flex flex-col gap-8">
            {section.rows.map((row) => (
              <li key={row.name} className="flex items-start gap-8">
                <span className="flex min-w-0 flex-1 flex-col gap-2">
                  <Mono className="text-13 text-text">{row.name}</Mono>
                  <span className="text-13 leading-normal text-text-2">
                    {row.line}
                  </span>
                </span>
                <span className="flex flex-col items-end gap-2">
                  <span className="text-14 font-semibold text-text tabular-nums">
                    {row.amount}
                  </span>
                  <span className="text-13 whitespace-nowrap text-text-2 tabular-nums">
                    {row.tokens}
                  </span>
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <HrefLink
        href={`${projectPath(slug, "agents")}?tab=cost`}
        className={cn(
          "self-start rounded-s text-13 font-medium text-accent hover:underline",
          focusRing,
        )}
      >
        Project cost
      </HrefLink>
    </SideGroup>
  );
}
