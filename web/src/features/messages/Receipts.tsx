import {
  Check,
  CheckCheck,
  CircleAlert,
  Clock,
  type LucideIcon,
} from "lucide-react";
import { Fragment, useId, useState } from "react";
import type { MessageView } from "@/api/messages";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import type { DeliveryTone } from "./delivery";
import { detailRow, receipts } from "./receiptLine";

const TONES: Record<DeliveryTone, { icon: LucideIcon; className: string }> = {
  failed: { icon: CircleAlert, className: "text-bad" },
  waiting: { icon: Clock, className: "text-warn" },
  forwarded: { icon: CircleAlert, className: "text-text-2" },
  delivered: { icon: Check, className: "text-text-2" },
  read: { icon: CheckCheck, className: "text-accent" },
};

/** The receipts of a message as one line; a folded line opens the per-recipient list in place. */
export function Receipts({
  message,
  now,
  you,
  align = "start",
}: {
  message: MessageView;
  now: number;
  you: string | null;
  align?: "start" | "end";
}) {
  const [open, setOpen] = useState(false);
  const listId = useId();
  const { parts, folded, details } = receipts(message, now, you);
  if (parts.length === 0) return null;
  const lead = parts[0]?.tone ?? "delivered";
  const LeadIcon = TONES[lead].icon;
  return (
    <div
      className={cn(
        "flex flex-col gap-4",
        align === "end" ? "items-end" : "items-start",
      )}
    >
      <p
        className={cn(
          "flex flex-wrap items-center gap-x-6 gap-y-2 text-13 text-text-2",
          align === "end" && "justify-end",
        )}
      >
        {folded ? (
          <>
            <LeadIcon
              aria-hidden="true"
              size={14}
              className={cn("shrink-0", TONES[lead].className)}
            />
            {parts.map((part, index) => (
              <Fragment key={index}>
                {index > 0 ? " · " : null}
                <span
                  className={
                    part.tone === "failed" || part.tone === "waiting"
                      ? TONES[part.tone].className
                      : undefined
                  }
                >
                  <ActorText segments={part.segments} you={you} />
                </span>
              </Fragment>
            ))}
            <button
              type="button"
              aria-expanded={open}
              aria-controls={listId}
              onClick={() => {
                setOpen(!open);
              }}
              className={cn(
                "inline-flex min-h-24 items-center rounded-s px-4 font-medium text-accent hover:underline",
                focusRing,
              )}
            >
              Details
            </button>
          </>
        ) : (
          parts.map((part, index) => {
            const { icon: Icon, className } = TONES[part.tone];
            return (
              <Fragment key={index}>
                {index > 0 ? (
                  <span aria-hidden="true" className="text-line">
                    |
                  </span>
                ) : null}
                <span
                  className={cn(
                    "inline-flex items-center gap-6",
                    (part.tone === "failed" || part.tone === "waiting") &&
                      className,
                  )}
                >
                  <Icon
                    aria-hidden="true"
                    size={14}
                    className={cn("shrink-0", className)}
                  />
                  <span>
                    <ActorText segments={part.segments} you={you} />
                  </span>
                </span>
              </Fragment>
            );
          })
        )}
      </p>
      {folded && open ? (
        <ul
          id={listId}
          aria-label="Delivery"
          className="flex w-full max-w-measure flex-col rounded-m bg-surface py-4 inset-ring inset-ring-line"
        >
          {details.map(detailRow).map((row) => {
            const { icon: Icon, className } = TONES[row.tone];
            return (
              <li
                key={row.actor}
                className="flex flex-wrap items-center gap-x-6 gap-y-2 px-12 py-4 text-13 text-text-2"
              >
                <Icon
                  aria-hidden="true"
                  size={14}
                  className={cn("shrink-0", className)}
                />
                {row.lead === "" ? null : <span>{row.lead}</span>}
                <PersonName name={row.actor} you={you} youWord />
                {row.after === null ? null : <span>{row.after}</span>}
                <span className="ml-auto">{row.trailing}</span>
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}
