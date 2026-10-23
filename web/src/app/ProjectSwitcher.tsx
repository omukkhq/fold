import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import * as Menu from "@radix-ui/react-dropdown-menu";
import { useQuery } from "@tanstack/react-query";
import { useRouter, useRouterState } from "@tanstack/react-router";
import { ArrowRight, ChevronDown, FolderGit2 } from "lucide-react";
import { projectsQuery } from "../api/queries";
import { switchProjectPath } from "../routes/nav";
import { menuContent, menuItem } from "../components/overlay/menuStyles";
import { Mono } from "../components/mono/Mono";
import { recentProjects } from "./projectSwitcherModel";

/** The current project and a menu of the most recently used ones, with the way to every project on the projects page. */
export function ProjectSwitcher({ slug }: { slug: string }) {
  const router = useRouter();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const projects = useQuery(projectsQuery());
  const recent = projects.data ? recentProjects(projects.data, slug) : [];

  return (
    <Menu.Root>
      <Menu.Trigger
        aria-label={`Project ${slug}, switch project`}
        className={cn(
          "flex min-w-0 items-center gap-6 rounded-m px-8 py-5 hover:bg-sunken max-lg:mr-auto max-lg:min-h-44 max-lg:min-w-44 max-lg:px-6",
          focusRing,
        )}
      >
        <FolderGit2
          size={15}
          className="shrink-0 text-text-muted max-lg:hidden"
          aria-hidden
        />
        <span className="truncate text-card font-semibold text-text">
          {slug}
        </span>
        <ChevronDown
          size={14}
          className="shrink-0 text-text-muted"
          aria-hidden
        />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Content align="start" sideOffset={6} className={menuContent}>
          {projects.isPending && (
            <div className="px-8 py-6 text-body text-text-muted">Loading</div>
          )}
          {projects.isError && (
            <div className="px-8 py-6 text-body text-danger">
              Could not load projects
            </div>
          )}
          {projects.data?.length === 0 && (
            <div className="px-8 py-6 text-body text-text-muted">
              No projects
            </div>
          )}
          <Menu.RadioGroup
            value={slug}
            onValueChange={(next) => {
              void router.navigate({ href: switchProjectPath(pathname, next) });
            }}
          >
            {recent.map((p) => (
              <Menu.RadioItem key={p.slug} value={p.slug} className={menuItem}>
                <span className="truncate">{p.name}</span>
                {p.detail !== null ? (
                  <Mono className="text-meta font-regular text-text-muted">
                    {p.detail}
                  </Mono>
                ) : null}
              </Menu.RadioItem>
            ))}
          </Menu.RadioGroup>
          <Menu.Separator className="my-4 h-px bg-border-subtle" />
          <Menu.Item
            className={cn(menuItem, "text-primary-text")}
            onSelect={() => {
              void router.navigate({ href: "/projects" });
            }}
          >
            See all
            <ArrowRight size={14} aria-hidden />
          </Menu.Item>
        </Menu.Content>
      </Menu.Portal>
    </Menu.Root>
  );
}
