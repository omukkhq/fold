import * as RadixPopover from "@radix-ui/react-popover";
import { useQuery } from "@tanstack/react-query";
import { Ellipsis } from "lucide-react";
import { useState, type ReactNode } from "react";
import { projectQuery } from "@/api/queries";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useLiveStatus } from "@/live/status";
import { isPaused } from "./agentChipsModel";
import { MORE_TRIGGER_CLASS, moreAlert, moreLabel } from "./topBarLayout";

/** The compact top bar's More button, below `lg`: the items that leave the row, in a popover so the pause keeps its pending state and error after the press. */
export function TopBarMore({
  slug,
  children,
}: {
  slug: string;
  /** The items, given the way to close the popover when one of them navigates. */
  children: (close: () => void) => ReactNode;
}) {
  const project = useQuery(projectQuery(slug));
  const status = useLiveStatus();
  const [open, setOpen] = useState(false);
  const alert = moreAlert(
    status.state,
    isPaused(project.data?.overview?.paused_since),
  );
  return (
    <RadixPopover.Root open={open} onOpenChange={setOpen}>
      <RadixPopover.Trigger asChild>
        <button
          type="button"
          aria-label={moreLabel(alert)}
          className={cn(
            "relative grid size-44 shrink-0 place-items-center rounded-m text-text-2 hover:bg-sunken",
            focusRing,
            MORE_TRIGGER_CLASS,
          )}
        >
          <Ellipsis aria-hidden="true" size={15} />
          {alert === null ? null : (
            <span
              aria-hidden="true"
              className={cn(
                "absolute top-10 right-10 size-7 rounded-full",
                alert.dot,
              )}
            />
          )}
        </button>
      </RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          align="end"
          sideOffset={6}
          collisionPadding={16}
          aria-label="More"
          className="z-10 flex w-agents max-w-(--width-popover-max) flex-col gap-12 rounded-m border border-line bg-surface p-12 shadow-pop outline-none"
        >
          {children(() => {
            setOpen(false);
          })}
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
