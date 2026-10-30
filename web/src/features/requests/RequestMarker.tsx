import { FilePlus2 } from "lucide-react";
import type { RequestSummary } from "@/api/requests";
import { HrefLink } from "@/app/HrefLink";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { projectPath } from "@/routes/paths";
import { requestMarkActive, requestMarkLabel } from "./codeLocation";

/** The mark at a line that a request made on code covers; it opens the request. */
export function RequestMarker({
  slug,
  request,
  focusable,
}: {
  slug: string;
  request: RequestSummary;
  /** Only one line of the request's lines takes Tab focus. */
  focusable: boolean;
}) {
  const label = requestMarkLabel(request);
  return (
    <HrefLink
      href={projectPath(slug, "requests", request.id)}
      aria-label={label}
      title={label}
      tabIndex={focusable ? 0 : -1}
      // The row under the marker selects its line on a click.
      onClick={(event) => {
        event.stopPropagation();
      }}
      className={cn(
        "inline-flex rounded-s",
        requestMarkActive(request) ? "text-accent" : "text-text-2",
        focusRing,
      )}
    >
      <FilePlus2 aria-hidden="true" size={13} />
    </HrefLink>
  );
}
