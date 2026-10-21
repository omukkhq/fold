import { ArrowLeft } from "lucide-react";
import {
  buttonClasses,
  buttonIconClass,
} from "../components/button/buttonStyles";
import { FailurePanel } from "../components/states/FailurePanel";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { HrefLink } from "./HrefLink";

/** The state of a project address that fold does not know, such as a project removed because its `.fold` folder was deleted. */
export function ProjectNotFound() {
  return (
    <GonePanel
      title="Project not found"
      text="fold has no project at this address. When the .fold folder of a project is deleted, fold removes the project. Open the project list to choose a project."
      hint={null}
    />
  );
}

/** The state of a project a person removed from fold, with the daemon's hint of how it comes back. */
export function ProjectRemoved({ hint }: { hint: string | null }) {
  return (
    <GonePanel
      title="Project removed from fold"
      text="A person removed this project from fold. Its data stays in the .fold folder of its folder. To add it back, search for it on the projects page and choose Add back."
      hint={hint}
    />
  );
}

function GonePanel({
  title,
  text,
  hint,
}: {
  title: string;
  text: string;
  hint: string | null;
}) {
  return (
    <div className="flex flex-col gap-16 px-24 pt-18 pb-24">
      <FailurePanel
        title={title}
        text={text}
        hint={hint}
        code="project_not_found"
        action={
          <HrefLink
            href="/projects"
            className={cn(buttonClasses("secondary", "md"), focusRing)}
          >
            <span className={buttonIconClass("secondary")}>
              <ArrowLeft aria-hidden="true" size={15} />
            </span>
            Back to projects
          </HrefLink>
        }
      />
    </div>
  );
}
