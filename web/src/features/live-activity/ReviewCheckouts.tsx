import { ScanEye } from "lucide-react";
import { cn } from "../../lib/cn";
import { focusRing } from "../../lib/focus";
import { PersonName } from "../../people/PersonName";
import { checkoutText } from "./eventStreamModel";
import { useReviewCheckouts } from "./useEventStream";

export function ReviewCheckouts({ slug }: { slug: string }) {
  const checkouts = useReviewCheckouts(slug);
  return (
    <section
      aria-label="Review checkouts"
      className="flex flex-col gap-8 rounded-m bg-surface px-14 py-14 sm:px-18"
    >
      <h2 className="flex items-center gap-8 text-16 font-semibold text-text">
        <ScanEye aria-hidden="true" size={16} className="text-text-2" />
        Review checkouts
      </h2>
      {checkouts.status === "pending" ? (
        <p role="status" className="text-14 text-text-2">
          Loading
        </p>
      ) : null}
      {checkouts.status === "error" ? (
        <p role="alert" className="flex items-center gap-8 text-14 text-text-2">
          Review checkouts could not load.
          <button
            type="button"
            onClick={checkouts.retry}
            className={cn("rounded-s text-accent underline", focusRing)}
          >
            Retry
          </button>
        </p>
      ) : null}
      {checkouts.status === "ready" && checkouts.rows.length === 0 ? (
        <p className="text-14 text-text-2">No review checkout is open.</p>
      ) : null}
      {checkouts.rows.length === 0 ? null : (
        <ul className="flex flex-col gap-6">
          {checkouts.rows.map((row) => (
            <li
              key={row.workspace}
              className="flex flex-wrap items-baseline gap-x-10 gap-y-2 text-14"
            >
              {row.worker === null ? (
                <span className="text-text-2">no worker</span>
              ) : (
                <PersonName name={row.worker} />
              )}
              <span className="min-w-0 text-text-2">{checkoutText(row)}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
