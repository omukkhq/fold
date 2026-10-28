import { Outlet } from "@tanstack/react-router";
import { MoveToasts } from "@/features/notifications/MoveToasts";
import { PausedBanner } from "./PausedBanner";
import { TopBar } from "./TopBar";

export function Shell({ slug }: { slug: string }) {
  return (
    <div className="relative flex h-dvh flex-col overflow-clip bg-bg">
      <TopBar slug={slug} />
      <PausedBanner slug={slug} />
      <main className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        <Outlet />
      </main>
      <MoveToasts slug={slug} />
    </div>
  );
}
