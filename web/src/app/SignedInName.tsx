import { useQuery } from "@tanstack/react-query";
import { UserRound } from "lucide-react";
import { sessionQuery } from "@/api/queries";
import { Popover } from "@/components/overlay/Popover";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { atName, humanActor } from "@/people/names";

/** The signed-in human's name; below `lg` only the icon shows, and the name opens from it. */
export function SignedInName() {
  const human = useQuery(sessionQuery()).data?.human ?? null;
  if (human === null) return null;
  const actor = humanActor(human);
  return (
    <Popover
      align="end"
      trigger={
        <button
          type="button"
          aria-label={`Signed in as ${atName(actor)}`}
          className={cn(
            "flex shrink-0 items-center gap-6 rounded-m px-8 py-5 text-text-2 hover:bg-sunken max-lg:size-44 max-lg:justify-center max-lg:p-0",
            focusRing,
          )}
        >
          <UserRound size={15} aria-hidden />
          <PersonName name={actor} className="hidden text-14 lg:inline" />
        </button>
      }
    >
      <p className="text-13 text-text-2">
        Signed in as <PersonName name={actor} className="text-14" />
      </p>
    </Popover>
  );
}
