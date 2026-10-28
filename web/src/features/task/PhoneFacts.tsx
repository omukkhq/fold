import type { TaskDetail } from "@/api/tasks";
import { Label } from "@/components/label/Label";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { ownerLine, taskLabelView } from "./areas";
import { useAreas } from "./LabelFields";

const FACT_LABEL =
  "pt-2 font-title text-13 leading-tight font-semibold text-text-2";

/** On a phone the side panel follows the messages, so who is responsible and the area come first, under the status. */
export function PhoneFacts({ slug, task }: { slug: string; task: TaskDetail }) {
  const { areas, failed, retry } = useAreas(slug);
  const holder = task.claims[0]?.holder ?? null;
  const view = areas === null ? null : taskLabelView(task.labels, areas);
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-12 gap-y-4 rounded-m bg-bg px-14 py-10 text-14">
      <dt className={FACT_LABEL}>Responsible</dt>
      <dd>
        {holder === null ? (
          <span className="text-text-2">Nobody yet</span>
        ) : (
          <PersonName name={holder} />
        )}
      </dd>
      <dt className={FACT_LABEL}>Area</dt>
      <dd className="flex flex-col gap-2">
        {failed ? (
          <span role="alert" className="flex flex-wrap items-center gap-x-8">
            The areas could not load.
            <button
              type="button"
              onClick={retry}
              className={cn("rounded-s text-accent underline", focusRing)}
            >
              Retry
            </button>
          </span>
        ) : view === null ? (
          <span className="text-text-2">Reading the areas…</span>
        ) : view.areas.length === 0 ? (
          <span className="text-text-2">Not chosen yet</span>
        ) : (
          view.areas.map((area) => (
            <span
              key={area.name}
              className="flex flex-wrap items-center gap-x-6"
            >
              <Label color={area.label.color} className="text-14 text-text">
                {area.label.name}
              </Label>
              <span className="text-13 text-text-2">{ownerLine(area)}</span>
            </span>
          ))
        )}
      </dd>
    </dl>
  );
}
