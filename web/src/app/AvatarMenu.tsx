import * as RadixPopover from "@radix-ui/react-popover";
import { useQuery } from "@tanstack/react-query";
import { UserRound } from "lucide-react";
import { sessionQuery } from "../api/queries";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { PersonName } from "../people/PersonName";
import { atName, humanActor } from "../people/names";
import { SignOut } from "./SignOut";
import { ThemeChoice } from "./ThemeChoice";
import { initials } from "./signOutModel";

/** The avatar at the end of the top bar: who is signed in, the theme and Sign out. */
export function AvatarMenu({ className }: { className?: string }) {
  const human = useQuery(sessionQuery()).data?.human ?? null;
  const actor = human === null ? null : humanActor(human);
  const letters = initials(human);
  return (
    <RadixPopover.Root>
      <RadixPopover.Trigger asChild>
        <button
          type="button"
          aria-label={
            actor === null
              ? "Your menu"
              : `Your menu, signed in as ${atName(actor)}`
          }
          className={cn(
            "grid size-32 shrink-0 place-items-center rounded-full bg-accent-soft font-title text-13 font-bold text-accent data-[state=open]:outline-2 data-[state=open]:outline-offset-2 data-[state=open]:outline-accent",
            focusRing,
            className,
          )}
        >
          {letters ?? <UserRound aria-hidden="true" size={16} />}
        </button>
      </RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          align="end"
          sideOffset={8}
          collisionPadding={16}
          aria-label="Your menu"
          className="z-pop flex w-menu max-w-(--width-popover-max) flex-col overflow-hidden rounded-m bg-surface pb-6 shadow-pop outline-none"
        >
          {actor === null ? null : (
            <p className="flex flex-col gap-2 px-16 pt-14 pb-6">
              <span className="text-13 text-text-2">Signed in as</span>
              <PersonName name={actor} className="text-14 font-semibold" />
            </p>
          )}
          <div className="flex flex-col gap-8 px-16 pt-8 pb-12">
            <span className="font-title text-13 font-semibold text-text-2">
              Theme
            </span>
            <ThemeChoice />
          </div>
          <SignOut />
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
