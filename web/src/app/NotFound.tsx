import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { HrefLink } from "./HrefLink";

export function NotFound() {
  return (
    <div className="flex flex-col items-start gap-8 p-24">
      <h1 className="text-16 font-semibold text-text">Not found</h1>
      <p className="text-15 text-text-2">
        This page or project does not exist.
      </p>
      <HrefLink
        href="/projects"
        className={cn(
          "text-15 font-medium text-accent hover:underline",
          focusRing,
        )}
      >
        Back to projects
      </HrefLink>
    </div>
  );
}
