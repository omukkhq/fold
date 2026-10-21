import { Outlet } from "@tanstack/react-router";
import { NavTabs } from "./NavTabs";
import { TopBar } from "./TopBar";

export function Shell({ slug }: { slug: string }) {
  return (
    <div className="flex h-dvh flex-col overflow-hidden bg-bg">
      <TopBar slug={slug} />
      <NavTabs slug={slug} />
      <main className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        <Outlet />
      </main>
    </div>
  );
}
