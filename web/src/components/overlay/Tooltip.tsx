import * as RadixTooltip from "@radix-ui/react-tooltip";
import type { ReactNode } from "react";

interface TooltipProps {
  /** The tooltip text. */
  content: string;
  /** The element the tooltip describes; it must take a ref and focus. */
  children: ReactNode;
}

export function Tooltip({ content, children }: TooltipProps) {
  return (
    <RadixTooltip.Provider delayDuration={300}>
      <RadixTooltip.Root>
        <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
        <RadixTooltip.Portal>
          <RadixTooltip.Content
            sideOffset={4}
            className="z-tooltip max-w-measure rounded-s bg-ink px-10 py-6 text-13 text-on-ink"
          >
            {content}
          </RadixTooltip.Content>
        </RadixTooltip.Portal>
      </RadixTooltip.Root>
    </RadixTooltip.Provider>
  );
}
