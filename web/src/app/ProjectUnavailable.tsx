import { ArrowLeft, RotateCw } from "lucide-react";
import { Button } from "../components/button/Button";
import {
  buttonClasses,
  buttonIconClass,
} from "../components/button/buttonStyles";
import { FailurePanel } from "../components/states/FailurePanel";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { HrefLink } from "./HrefLink";
import type { ProjectProblem } from "./projectGone";

const TEXTS = {
  folder_missing: {
    title: "Project folder missing",
    text: "fold cannot find the folder of this project, as when its disk is not mounted. fold keeps the project and opens it again when the folder is back.",
  },
  database_missing: {
    title: "Project database missing",
    text: "The .fold folder of this project has no project database. Restore the database from a backup, or delete the .fold folder and run `fld init` again to make a new project there.",
  },
  unavailable: {
    title: "Project unavailable",
    text: "fold could not open this project. Run `fld doctor` to see why.",
  },
} as const;

/** The state of a registered project that the daemon cannot open now. */
export function ProjectUnavailable({
  problem,
  onRetry,
}: {
  problem: Exclude<ProjectProblem, { kind: "gone" | "removed" }>;
  onRetry: () => void;
}) {
  const { title, text } = TEXTS[problem.kind];
  return (
    <div className="flex flex-col gap-16 px-24 pt-18 pb-24">
      <FailurePanel
        title={title}
        text={text}
        hint={problem.hint}
        code="project_unavailable"
        action={
          <div className="flex shrink-0 gap-8">
            <Button
              variant="quiet"
              icon={<RotateCw aria-hidden="true" size={16} />}
              onClick={onRetry}
            >
              Retry
            </Button>
            <HrefLink
              href="/projects"
              className={cn(buttonClasses("quiet", "md"), focusRing)}
            >
              <span className={buttonIconClass("quiet")}>
                <ArrowLeft aria-hidden="true" size={16} />
              </span>
              Back to projects
            </HrefLink>
          </div>
        }
      />
    </div>
  );
}
