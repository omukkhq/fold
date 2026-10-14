import * as RadixTabs from "@radix-ui/react-tabs";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { TAB_BY_STATE, TAB_LIST } from "./tabStyles";

export interface TabItem {
  value: string;
  label: string;
  icon?: LucideIcon;
  content: ReactNode;
}

interface TabsProps {
  items: TabItem[];
  /** The tab shown first when the component keeps its own state. */
  defaultValue?: string;
  value?: string;
  onValueChange?: (value: string) => void;
  "aria-label": string;
}

export function Tabs({
  items,
  defaultValue,
  value,
  onValueChange,
  "aria-label": label,
}: TabsProps) {
  const first = items[0]?.value;
  return (
    <RadixTabs.Root
      defaultValue={value === undefined ? (defaultValue ?? first) : undefined}
      value={value}
      onValueChange={onValueChange}
    >
      <RadixTabs.List aria-label={label} className={TAB_LIST}>
        {items.map((item) => {
          const Icon = item.icon;
          return (
            <RadixTabs.Trigger
              key={item.value}
              value={item.value}
              className={cn(TAB_BY_STATE, focusRing)}
            >
              {Icon ? <Icon aria-hidden="true" size={15} /> : null}
              {item.label}
            </RadixTabs.Trigger>
          );
        })}
      </RadixTabs.List>
      {items.map((item) => (
        <RadixTabs.Content
          key={item.value}
          value={item.value}
          className={focusRing}
        >
          {item.content}
        </RadixTabs.Content>
      ))}
    </RadixTabs.Root>
  );
}
