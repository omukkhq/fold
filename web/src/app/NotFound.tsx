import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { HrefLink } from "./HrefLink";

export function NotFound() {
  return (
    <div className="flex flex-col items-start gap-8 p-24">
      <h1 className="text-section font-semibold text-text">Not found</h1>
      <p className="text-body text-text-muted">
        This page or project does not exist.
      </p>
      <HrefLink
        href="/projects"
        className={cn(
          "text-body font-medium text-primary-text hover:underline",
          focusRing,
        )}
      >
        Back to projects
      </HrefLink>
    </div>
  );
}
