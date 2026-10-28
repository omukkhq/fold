import { ArrowLeft } from "lucide-react";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { FailurePanel } from "@/components/states/FailurePanel";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { HrefLink } from "./HrefLink";
import { missingState, type Missing } from "./missingState";

/** The not-found state in the content area of an open project, under its top bar and tabs. */
export function MissingPanel({
  slug,
  missing,
}: {
  slug: string;
  missing: Missing;
}) {
  const state = missingState(slug, missing);
  return (
    <div className="flex min-w-0 flex-col gap-16 px-16 pt-20 pb-24 md:px-24">
      <FailurePanel
        title={state.title}
        text={state.text}
        code={state.code}
        action={
          <HrefLink
            href={state.href}
            className={cn(buttonClasses("secondary", "md"), focusRing)}
          >
            <span className={buttonIconClass("secondary")}>
              <ArrowLeft aria-hidden="true" size={15} />
            </span>
            {state.linkText}
          </HrefLink>
        }
      />
    </div>
  );
}
