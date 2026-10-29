import { SearchX } from "lucide-react";
import { buttonClasses } from "../components/button/buttonStyles";
import { EmptyPanel } from "../components/states/EmptyPanel";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { HrefLink } from "./HrefLink";

export function NotFound() {
  return (
    <div className="p-24">
      <EmptyPanel
        icon={SearchX}
        title="Not found"
        action={
          <HrefLink
            href="/projects"
            className={cn(buttonClasses("quiet", "md"), focusRing)}
          >
            Back to projects
          </HrefLink>
        }
      >
        This page or project does not exist.
      </EmptyPanel>
    </div>
  );
}
