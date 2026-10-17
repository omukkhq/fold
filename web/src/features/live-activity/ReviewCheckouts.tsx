import { ScanEye } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { PersonName } from "../../people/PersonName";
import { checkoutText } from "./eventStreamModel";
import { useReviewCheckouts } from "./useEventStream";

const ROW = "flex flex-wrap items-center gap-12 px-16 py-12";

/** Every row of the strip starts with the title; a later row keeps its width invisibly so the workers line up. */
function Title({ hidden }: { hidden: boolean }) {
  return (
    <>
      <ScanEye
        aria-hidden="true"
        size={15}
        className={cn("text-text-muted", hidden && "invisible")}
      />
      <span
        aria-hidden={hidden}
        className={cn(
          "text-bar font-semibold text-text",
          hidden && "invisible",
        )}
      >
        Review checkouts
      </span>
    </>
  );
}

function Message({ children }: { children: ReactNode }) {
  return (
    <div className={ROW}>
      <Title hidden={false} />
      {children}
    </div>
  );
}

export function ReviewCheckouts({ slug }: { slug: string }) {
  const checkouts = useReviewCheckouts(slug);
  return (
    <section
      aria-label="Review checkouts"
      className="flex flex-col divide-y divide-border-subtle rounded-l bg-surface inset-ring inset-ring-border-subtle"
    >
      {checkouts.status === "pending" ? (
        <Message>
          <p role="status" className="text-bar text-text-muted">
            Loading
          </p>
        </Message>
      ) : null}
      {checkouts.status === "error" ? (
        <Message>
          <p
            role="alert"
            className="flex items-center gap-8 text-bar text-text-muted"
          >
            Review checkouts could not load.
            <button
              type="button"
              onClick={checkouts.retry}
              className="rounded-s text-primary-text underline"
            >
              Retry
            </button>
          </p>
        </Message>
      ) : null}
      {checkouts.status === "ready" && checkouts.rows.length === 0 ? (
        <Message>
          <p className="text-bar text-text-muted">
            No review checkout is open.
          </p>
        </Message>
      ) : null}
      {checkouts.rows.map((row, index) => (
        <div key={row.workspace} className={ROW}>
          <Title hidden={index > 0} />
          <span className="flex items-center gap-6 font-mono text-meta text-text">
            {row.worker === null ? (
              "no worker"
            ) : (
              <PersonName name={row.worker} className="text-bar" />
            )}
          </span>
          <p className="min-w-0 flex-1 text-bar text-text-muted">
            {checkoutText(row)}
          </p>
        </div>
      ))}
    </section>
  );
}
