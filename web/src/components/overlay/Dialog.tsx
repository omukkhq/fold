import * as RadixDialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { IconButton } from "../button/Button";
import {
  dialogBody,
  dialogContent,
  dialogFill,
  dialogFillOverlay,
  dialogOverlay,
} from "./dialogStyles";

interface DialogProps {
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** The element that opens the dialog; Radix returns focus to it on close. */
  trigger?: ReactNode;
  title: string;
  description?: string;
  children?: ReactNode;
  /** The dialog takes the reading width of a long text, and on a phone the whole screen. */
  wide?: boolean;
}

export function Dialog({
  open,
  onOpenChange,
  trigger,
  title,
  description,
  children,
  wide = false,
}: DialogProps) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger ? (
        <RadixDialog.Trigger asChild>{trigger}</RadixDialog.Trigger>
      ) : null}
      <RadixDialog.Portal>
        <RadixDialog.Overlay
          className={cn(dialogOverlay, wide && dialogFillOverlay)}
        >
          <RadixDialog.Content
            className={cn(
              dialogContent,
              wide ? cn("w-dialog", dialogFill) : "w-max",
            )}
          >
            <div className="flex shrink-0 items-start justify-between gap-12 px-20 pt-20 pb-12">
              <div className="flex min-w-0 flex-col gap-4">
                <RadixDialog.Title className="text-16 font-semibold text-text">
                  {title}
                </RadixDialog.Title>
                {description ? (
                  <RadixDialog.Description className="text-15 text-text-2">
                    {description}
                  </RadixDialog.Description>
                ) : null}
              </div>
              <RadixDialog.Close asChild>
                <IconButton aria-label="Close" size="sm">
                  <X aria-hidden="true" size={14} />
                </IconButton>
              </RadixDialog.Close>
            </div>
            <div className={dialogBody}>
              <div className="flex flex-col gap-12 px-20 pb-20">{children}</div>
            </div>
          </RadixDialog.Content>
        </RadixDialog.Overlay>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
