import * as RadixPopover from "@radix-ui/react-popover";
import type { ReactNode } from "react";

interface PopoverProps {
  /** The element that opens the popover. */
  trigger: ReactNode;
  children: ReactNode;
  align?: "start" | "center" | "end";
}

export function Popover({ trigger, children, align = "start" }: PopoverProps) {
  return (
    <RadixPopover.Root>
      <RadixPopover.Trigger asChild>{trigger}</RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          align={align}
          sideOffset={4}
          className="z-10 rounded-m border border-border bg-raised p-12 text-body text-text shadow-card outline-none"
        >
          {children}
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
