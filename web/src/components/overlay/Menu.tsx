import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { menuContent, menuItem } from "./menuStyles";

export interface MenuItem {
  label: string;
  icon?: LucideIcon;
  onSelect: () => void;
  danger?: boolean;
  disabled?: boolean;
}

interface MenuProps {
  /** The element that opens the menu. */
  trigger: ReactNode;
  items: MenuItem[];
  align?: "start" | "center" | "end";
  /** A line above the items that says what choosing one does. */
  hint?: string;
}

/** The menu keeps a space of 8 to the window's edges and never grows past the room the window leaves it, so it fits at phone width too. */
export function Menu({ trigger, items, align = "start", hint }: MenuProps) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>{trigger}</DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align={align}
          sideOffset={4}
          collisionPadding={8}
          className={cn(
            menuContent,
            "max-h-(--radix-dropdown-menu-content-available-height) w-max max-w-(--radix-dropdown-menu-content-available-width) overflow-y-auto",
          )}
        >
          {hint === undefined ? null : (
            <DropdownMenu.Label className="max-w-measure px-10 pt-6 pb-4 text-13 text-text-2">
              {hint}
            </DropdownMenu.Label>
          )}
          {items.map((item) => {
            const Icon = item.icon;
            return (
              <DropdownMenu.Item
                key={item.label}
                disabled={item.disabled}
                onSelect={item.onSelect}
                className={cn(
                  menuItem,
                  "data-[disabled]:opacity-60",
                  item.danger && "text-bad",
                )}
              >
                {Icon ? <Icon aria-hidden="true" size={16} /> : null}
                {item.label}
              </DropdownMenu.Item>
            );
          })}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
