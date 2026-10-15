import { Badge } from "@/components/badge/Badge";
import { PersonName } from "./PersonName";
import { reviewerActor } from "./names";

/** The human-review mark as a badge: `Human review`, and the named reviewer as `@name` after it. */
export function HumanReviewBadge({
  reviewer,
  className,
}: {
  /** The reviewer's bare name; null when any human can approve. */
  reviewer: string | null;
  className?: string;
}) {
  const actor = reviewerActor(reviewer);
  return (
    <Badge
      tone="human-review"
      className={className}
      after={
        actor === null ? null : (
          <>
            <span aria-hidden="true">·</span>
            <PersonName name={actor} />
          </>
        )
      }
    >
      Human review
    </Badge>
  );
}
