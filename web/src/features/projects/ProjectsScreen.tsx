import { useSuspenseQuery } from "@tanstack/react-query";
import { ChevronRight, FolderGit2 } from "lucide-react";
import { useState } from "react";
import { projectsQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { ThemeToggle } from "@/app/ThemeToggle";
import { Avatar } from "@/components/avatar/Avatar";
import { Mono } from "@/components/mono/Mono";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { lastChangeLine, summaryLine } from "./counts";

const ROW =
  "flex items-center gap-20 rounded-l bg-surface px-20 py-16 inset-ring inset-ring-border-subtle";

export function ProjectsScreen() {
  const { data: projects } = useSuspenseQuery(projectsQuery());
  // The clock is read once per mount: the change times are a snapshot.
  const [now] = useState(() => Date.now());
  return (
    <div className="flex min-h-screen flex-col bg-bg">
      <header className="flex h-bar items-center justify-between border-b border-border-subtle bg-surface px-24">
        <span className="text-wordmark font-bold text-text">fold</span>
        <ThemeToggle />
      </header>
      <main className="flex flex-col px-40 py-40">
        <div className="mx-auto flex w-content max-w-full flex-col gap-20">
          <div className="flex flex-wrap items-end justify-between gap-16">
            <div className="flex flex-col gap-4">
              <h1 className="text-page leading-snug font-semibold text-text">
                Projects
              </h1>
              <p className="text-body text-text-muted">
                Every project this fold daemon serves on this machine.
              </p>
            </div>
            <p className="flex items-center gap-8 rounded-m bg-sunken px-12 py-7 text-bar text-text-muted">
              Add a project:
              <Mono className="text-text">fold init &lt;dir&gt;</Mono>
            </p>
          </div>
          {projects.length === 0 ? (
            <EmptyPanel
              icon={FolderGit2}
              title="No projects yet"
              action={<Mono className="text-text">fold init &lt;dir&gt;</Mono>}
            >
              Run fold init in a project directory. It appears here when the
              daemon has registered it.
            </EmptyPanel>
          ) : (
            <ul aria-label="Projects" className="flex flex-col gap-12">
              {projects.map((project) => {
                const { overview } = project;
                const content = (
                  <>
                    <div className="flex min-w-0 flex-1 flex-col gap-4">
                      <span className="flex items-center gap-10">
                        <span className="text-item-title leading-snug font-semibold text-text">
                          {project.name}
                        </span>
                        <Mono className="text-meta text-text-muted">
                          {project.slug}
                        </Mono>
                      </span>
                      <span className="text-bar text-text-muted">
                        {summaryLine(project)}
                      </span>
                    </div>
                    {overview ? (
                      <>
                        <div className="flex items-center gap-6">
                          {overview.agents.length === 0 ? (
                            <span className="text-code text-text-muted">
                              no agents connected
                            </span>
                          ) : (
                            overview.agents.map((agent) => (
                              <span
                                key={agent.name}
                                className="flex items-center gap-5 rounded-full py-2 pr-8 pl-3 inset-ring inset-ring-border"
                              >
                                <Avatar name={agent.name} />
                                <span className="text-code text-text">
                                  {agent.name}
                                </span>
                              </span>
                            ))
                          )}
                        </div>
                        <span className="flex items-center gap-6 text-code">
                          <span
                            aria-hidden
                            className={cn(
                              "size-7 rounded-full",
                              overview.agents.length > 0
                                ? "bg-primary"
                                : "bg-status-backlog",
                            )}
                          />
                          <span
                            className={
                              overview.agents.length > 0
                                ? "text-primary-text"
                                : "text-text-muted"
                            }
                          >
                            {lastChangeLine(overview, now)}
                          </span>
                        </span>
                        <ChevronRight
                          aria-hidden
                          size={16}
                          className="text-text-faint"
                        />
                      </>
                    ) : null}
                  </>
                );
                return (
                  <li key={project.slug}>
                    {project.available ? (
                      <HrefLink
                        href={`/p/${project.slug}/board`}
                        className={cn(ROW, "hover:bg-sunken", focusRing)}
                      >
                        {content}
                      </HrefLink>
                    ) : (
                      <div className={cn(ROW, "opacity-60")}>{content}</div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </main>
    </div>
  );
}
