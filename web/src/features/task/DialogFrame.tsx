import * as RadixDialog from "@radix-ui/react-dialog";
import { ChevronRight, X } from "lucide-react";
import type { ReactNode } from "react";
import { IconButton } from "@/components/button/Button";
import { cn } from "@/lib/cn";

interface Props {
  /** The last crumb: the task id, or `New task`. */
  crumb: string;
  /** An id takes the mono face; a word does not. */
  crumbIsId: boolean;
  /** Head actions before Close. */
  actions?: ReactNode;
  /** Whether the body renders a `Dialog.Description`; Radix warns when the content names one that is absent. */
  described: boolean;
  /**
   * Whether the body is the task's two columns, which scroll on their own from `md` up. Below `md` the columns
   * stack, so the body scrolls as one there, as every other body does.
   */
  split?: boolean;
  onClose: () => void;
  children: ReactNode;
}

/** The dialog over the board: the crumb head (the project is in the top bar already) with its actions stays in place, and only the body scrolls. */
export function DialogFrame({
  crumb,
  crumbIsId,
  actions,
  described,
  split = false,
  onClose,
  children,
}: Props) {
  return (
    <RadixDialog.Root
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <RadixDialog.Portal>
        {/* z-10 lifts the dialog over the board's raised marks; menus and popovers share it and open later in the DOM, so they stay on top. */}
        <RadixDialog.Overlay className="fixed inset-0 z-10 flex justify-center bg-scrim px-16 py-32">
          <RadixDialog.Content
            {...(described ? {} : { "aria-describedby": undefined })}
            className="flex max-h-full w-dialog max-w-full flex-col self-start overflow-hidden rounded-l bg-surface shadow-dialog inset-ring inset-ring-border outline-none"
          >
            <div className="flex shrink-0 items-center justify-between gap-12 border-b border-border-subtle py-12 pr-16 pl-24">
              <nav
                aria-label="Breadcrumb"
                className="flex items-center gap-8 text-bar text-text-muted"
              >
                <span>Board</span>
                <ChevronRight
                  aria-hidden="true"
                  size={13}
                  className="text-text-faint"
                />
                <span className={cn("text-text", crumbIsId && "font-mono")}>
                  {crumb}
                </span>
              </nav>
              <div className="flex items-center gap-4">
                {actions}
                <RadixDialog.Close asChild>
                  <IconButton aria-label="Close">
                    <X aria-hidden="true" size={15} />
                  </IconButton>
                </RadixDialog.Close>
              </div>
            </div>
            <div
              className={cn(
                "min-h-0",
                split
                  ? "max-md:overflow-y-auto md:flex md:flex-col"
                  : "overflow-y-auto",
              )}
            >
              {children}
            </div>
          </RadixDialog.Content>
        </RadixDialog.Overlay>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
