import { Outlet } from "@tanstack/react-router";
import { NavTabs } from "./NavTabs";
import { TopBar } from "./TopBar";

export function Shell({ slug }: { slug: string }) {
  return (
    <div className="flex min-h-screen flex-col bg-bg">
      <TopBar slug={slug} />
      <NavTabs slug={slug} />
      <main className="flex flex-1 flex-col">
        <Outlet />
      </main>
    </div>
  );
}
