import { ArrowLeft, SearchX } from "lucide-react";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { HrefLink } from "./HrefLink";
import { missingState, type Missing } from "./missingState";

/** The not-found state in the content area of an open project, under its top bar and tabs: a neutral block, because an address the project lacks is no failure. */
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
      <EmptyPanel
        icon={SearchX}
        title={state.title}
        action={
          <HrefLink
            href={state.href}
            className={cn(buttonClasses("quiet", "md"), focusRing)}
          >
            <span className={buttonIconClass("quiet")}>
              <ArrowLeft aria-hidden="true" size={16} />
            </span>
            {state.linkText}
          </HrefLink>
        }
      >
        {state.text}
      </EmptyPanel>
    </div>
  );
}
