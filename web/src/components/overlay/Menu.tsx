import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { Check, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { menuContent, menuItem } from "./menuStyles";

export interface MenuItem {
  label: string;
  icon?: LucideIcon;
  onSelect: () => void;
  danger?: boolean;
  disabled?: boolean;
  /** Makes the item a tick box. Ticking keeps the menu open, so a reader ticks several entries in one visit. */
  checked?: boolean;
  /** Quiet text after the label, such as the area of an area lead. */
  hint?: string;
}

interface MenuProps {
  /** The element that opens the menu. */
  trigger: ReactNode;
  items: MenuItem[];
  align?: "start" | "center" | "end";
}

/** The room Radix measures stops short of the window edge, so the last item of a long menu never touches it. */
const EDGE = 8;

function ItemBody({ item }: { item: MenuItem }) {
  const Icon = item.icon;
  return (
    <>
      {Icon ? <Icon aria-hidden="true" size={16} /> : null}
      <span className="min-w-0 wrap-anywhere">{item.label}</span>
      {item.hint === undefined ? null : (
        <>
          {" "}
          <span className="ml-auto min-w-0 pl-8 text-13 font-regular wrap-anywhere text-text-2">
            {item.hint}
          </span>
        </>
      )}
    </>
  );
}

export function Menu({ trigger, items, align = "start" }: MenuProps) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>{trigger}</DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align={align}
          sideOffset={4}
          collisionPadding={EDGE}
          className={cn(
            menuContent,
            "w-max max-w-(--radix-dropdown-menu-content-available-width)",
          )}
        >
          {items.map((item) => {
            const className = cn(
              menuItem,
              "data-[disabled]:opacity-60",
              item.danger && "text-bad",
            );
            if (item.checked === undefined) {
              return (
                <DropdownMenu.Item
                  key={item.label}
                  disabled={item.disabled}
                  onSelect={item.onSelect}
                  className={className}
                >
                  <ItemBody item={item} />
                </DropdownMenu.Item>
              );
            }
            return (
              <DropdownMenu.CheckboxItem
                key={item.label}
                checked={item.checked}
                disabled={item.disabled}
                onCheckedChange={item.onSelect}
                onSelect={(event) => {
                  event.preventDefault();
                }}
                className={className}
              >
                <span className="inline-flex size-16 shrink-0 items-center justify-center">
                  <DropdownMenu.ItemIndicator>
                    <Check aria-hidden="true" size={16} />
                  </DropdownMenu.ItemIndicator>
                </span>
                <ItemBody item={item} />
              </DropdownMenu.CheckboxItem>
            );
          })}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
