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
  onClose: () => void;
  children: ReactNode;
}

/** The dialog over the board: the crumb head (the project is in the top bar already) with its actions, and a body that scrolls with the page. */
export function DialogFrame({
  crumb,
  crumbIsId,
  actions,
  described,
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
        <RadixDialog.Overlay className="fixed inset-0 z-10 flex justify-center overflow-y-auto bg-scrim px-16 py-32">
          <RadixDialog.Content
            {...(described ? {} : { "aria-describedby": undefined })}
            className="flex h-max w-dialog max-w-full flex-col rounded-l bg-surface shadow-dialog inset-ring inset-ring-line outline-none"
          >
            <div className="flex items-center justify-between gap-12 border-b border-line py-12 pr-16 pl-24">
              <nav
                aria-label="Breadcrumb"
                className="flex items-center gap-8 text-14 text-text-2"
              >
                <span>Board</span>
                <ChevronRight
                  aria-hidden="true"
                  size={13}
                  className="text-text-2"
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
            {children}
          </RadixDialog.Content>
        </RadixDialog.Overlay>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
