import { useQuery } from "@tanstack/react-query";
import { CirclePause } from "lucide-react";
import { projectQuery } from "@/api/queries";
import { cn } from "@/lib/cn";
import { useNow } from "@/live/useNow";
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
    // The bottom padding is 1px less than the top, so the 1px border stays inside the banner's height like an inner stroke.
    <div
      role="status"
      className={cn(
        "flex items-center gap-10 border-b border-border-subtle bg-warning-soft px-24 pt-9 pb-8",
        className,
      )}
    >
      <CirclePause
        aria-hidden="true"
        size={15}
        className="shrink-0 text-warning"
      />
      <p className="text-bar leading-snug font-semibold text-text">
        {text.title}
      </p>
      <p className="min-w-0 flex-1 text-bar leading-snug text-secondary-text">
        {text.detail}
      </p>
    </div>
  );
}
