import * as RadixDialog from "@radix-ui/react-dialog";
import { ChevronRight, X } from "lucide-react";
import type { ReactNode } from "react";
import { IconButton } from "@/components/button/Button";
import { Id } from "@/components/id/Id";
import {
  dialogContent,
  dialogFill,
  dialogFillOverlay,
  dialogOverlay,
} from "@/components/overlay/dialogStyles";
import { cn } from "@/lib/cn";

interface Props {
  /** The last crumb: the task id, or `New task`. */
  crumb: string;
  /** An id keeps its digits even and never wraps. */
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
  /** The create form is one column, so the dialog is narrower. */
  narrow?: boolean;
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
  narrow = false,
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
        <RadixDialog.Overlay className={cn(dialogOverlay, dialogFillOverlay)}>
          <RadixDialog.Content
            {...(described ? {} : { "aria-describedby": undefined })}
            // The dialog itself takes the focus on open, so no control shows a ring before the human moves.
            onOpenAutoFocus={(event) => {
              const content = event.currentTarget;
              if (
                content instanceof HTMLElement &&
                !content.contains(document.activeElement)
              ) {
                event.preventDefault();
                content.focus();
              }
            }}
            className={cn(
              dialogContent,
              dialogFill,
              "self-start focus-visible:outline-2 focus-visible:outline-offset-3 focus-visible:outline-accent max-md:focus-visible:-outline-offset-2",
              narrow ? "w-dialog-narrow" : "w-dialog",
            )}
          >
            <div className="flex shrink-0 items-center justify-between gap-12 border-b border-line py-10 pr-14 pl-24">
              <nav
                aria-label="Breadcrumb"
                className="flex min-w-0 items-center gap-6 text-14 text-text-2"
              >
                <span>Board</span>
                <ChevronRight
                  aria-hidden="true"
                  size={14}
                  className="shrink-0 text-text-2"
                />
                {crumbIsId ? (
                  <Id className="font-semibold text-text">{crumb}</Id>
                ) : (
                  <span className="font-semibold text-text">{crumb}</span>
                )}
              </nav>
              <div className="flex items-center gap-4">
                {actions}
                <RadixDialog.Close asChild>
                  <IconButton aria-label="Close">
                    <X aria-hidden="true" size={16} />
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
