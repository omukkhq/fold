import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import * as Menu from "@radix-ui/react-dropdown-menu";
import { useQuery } from "@tanstack/react-query";
import { useRouter, useRouterState } from "@tanstack/react-router";
import { ChevronDown, FolderGit2 } from "lucide-react";
import { projectsQuery } from "../api/queries";
import { switchProjectPath } from "../routes/nav";
import { menuContent, menuItem } from "../components/overlay/menuStyles";

export function ProjectSwitcher({ slug }: { slug: string }) {
  const router = useRouter();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const projects = useQuery(projectsQuery());

  return (
    <Menu.Root>
      <Menu.Trigger
        aria-label={`Project ${slug}, switch project`}
        className={cn(
          "flex items-center gap-6 rounded-m px-8 py-5 hover:bg-sunken",
          focusRing,
        )}
      >
        <FolderGit2 size={15} className="text-text-muted" aria-hidden />
        <span className="text-card font-semibold text-text">{slug}</span>
        <ChevronDown size={14} className="text-text-muted" aria-hidden />
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
            {projects.data?.map((p) => (
              <Menu.RadioItem key={p.slug} value={p.slug} className={menuItem}>
                {p.name}
              </Menu.RadioItem>
            ))}
          </Menu.RadioGroup>
        </Menu.Content>
      </Menu.Portal>
    </Menu.Root>
  );
}
