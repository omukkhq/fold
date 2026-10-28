import { useQuery } from "@tanstack/react-query";
import { CirclePause } from "lucide-react";
import { projectQuery } from "@/api/queries";
import { cn } from "@/lib/cn";
import { useNow } from "@/live/useNow";
import { ActorText } from "@/people/ActorText";
import { pausedBanner } from "./pause";

/** The band that says the project's agents are paused, since when and by whom; nothing while they run. */
export function PausedBanner({
  slug,
  className,
}: {
  slug: string;
  className?: string;
}) {
  const project = useQuery(projectQuery(slug));
  const overview = project.data?.overview ?? null;
  const paused = overview !== null && overview.paused_since !== null;
  const now = useNow(paused, 60_000);
  const text =
    overview === null
      ? null
      : pausedBanner(now, overview.paused_since, overview.paused_by);
  if (text === null) {
    return null;
  }
  return (
    <div
      role="status"
      className={cn(
        "flex shrink-0 flex-wrap items-baseline gap-x-12 gap-y-2 bg-warn-soft px-16 py-10 wide:px-22",
        className,
      )}
    >
      <CirclePause
        aria-hidden="true"
        size={16}
        className="shrink-0 self-center text-warn"
      />
      <p className="text-14 font-semibold text-text">
        <ActorText segments={text.title} />
      </p>
      <p className="min-w-0 flex-1 basis-2/3 text-14 text-text-2">
        {text.detail}
      </p>
    </div>
  );
}
