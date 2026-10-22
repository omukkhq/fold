import { useSuspenseQuery } from "@tanstack/react-query";
import { ChevronRight, FolderGit2, Search, SearchX } from "lucide-react";
import { useState } from "react";
import type { ProjectSummary } from "@/api/projects";
import { CodeText } from "@/components/code/CodeText";
import { useNow } from "@/live/useNow";
import { projectsQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { ThemeToggle } from "@/app/ThemeToggle";
import { Mono } from "@/components/mono/Mono";
import { Wordmark } from "@/components/Wordmark";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { allProjectsCost, projectCostLines } from "../cost/cost";
import { GitStatus } from "../git/GitStatus";
import { lastChangeLine, summaryLine } from "./counts";
import { AddBackProject } from "./AddBackProject";
import { ProjectAgentChips } from "./ProjectAgentChips";
import {
  addBackBlocker,
  projectActions,
  projectMark,
  showsRoot,
  shownProjects,
} from "./projectsHome";
import { RemoveProject } from "./RemoveProject";
import { projectPath } from "@/routes/paths";

/** At phone width the agents and the change time go below the name, each on its own line; from `md` they are columns beside it. */
const LINE = "flex flex-col gap-10 md:flex-row md:items-center md:gap-20";
const ROW =
  "rounded-l bg-surface py-16 pr-12 pl-20 inset-ring inset-ring-border-subtle";
const CARD =
  "flex flex-col overflow-hidden rounded-l bg-surface inset-ring inset-ring-border-subtle";

export function ProjectsScreen() {
  const { data: projects } = useSuspenseQuery(projectsQuery());
  const total = allProjectsCost(projects);
  // The clock is read once per mount: the change times are a snapshot.
  const now = useNow(true, 30_000);
  const [query, setQuery] = useState("");
  const shown = shownProjects(projects, query);
  const searching = query.trim() !== "";
  return (
    <div className="flex min-h-screen flex-col bg-bg">
      <header className="flex h-bar items-center justify-between border-b border-border-subtle bg-surface px-16 md:px-24">
        <Wordmark />
        <ThemeToggle />
      </header>
      <main className="flex flex-col px-16 py-24 md:px-24 md:py-40">
        <div className="flex min-w-0 flex-col gap-20">
          <div className="flex flex-wrap items-end justify-between gap-16">
            <div className="flex flex-col gap-4">
              <h1 className="text-page leading-snug font-semibold text-text">
                Projects
              </h1>
              <p className="text-body text-text-muted">
                Every project this fold daemon serves on this machine.
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-16">
              {projects.length > 0 ? (
                <p className="flex flex-col md:items-end">
                  <span className="text-meta text-text-muted">
                    Cost of all projects
                  </span>
                  <span className="text-item-title leading-snug font-semibold text-text tabular-nums">
                    {total.cost}
                  </span>
                  <span className="text-meta text-text-muted tabular-nums">
                    {total.tokens}
                  </span>
                </p>
              ) : null}
              <p className="flex items-center gap-8 rounded-m bg-sunken px-12 py-7 text-bar text-text-muted">
                Add a project:
                <Mono className="text-text">fld init &lt;dir&gt;</Mono>
              </p>
            </div>
          </div>
          {projects.length > 0 ? (
            <label className="flex items-center gap-8 rounded-m bg-surface px-12 py-9 inset-ring inset-ring-border has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-2 has-[input:focus-visible]:outline-focus-ring">
              <Search
                aria-hidden="true"
                size={15}
                className="shrink-0 text-text-faint"
              />
              <input
                type="search"
                aria-label="Search projects"
                placeholder="Search by name, slug or folder"
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                }}
                className="min-w-0 flex-1 bg-transparent text-body leading-snug text-text outline-none placeholder:text-text-faint"
              />
            </label>
          ) : null}
          {shown.length === 0 && searching ? (
            <EmptyPanel icon={SearchX} title="No project matches">
              No project in fold or removed from it has this text in its name,
              slug or folder.
            </EmptyPanel>
          ) : shown.length === 0 ? (
            <EmptyPanel
              icon={FolderGit2}
              title="No projects yet"
              action={<Mono className="text-text">fld init &lt;dir&gt;</Mono>}
            >
              Run `fld init` in a project directory. It appears here when the
              daemon has registered it.
            </EmptyPanel>
          ) : (
            <ul
              aria-label={searching ? "Search results" : "Projects"}
              className="flex flex-col gap-12"
            >
              {shown.map((project) => (
                <li key={project.slug}>
                  <ProjectCard project={project} now={now} query={query} />
                </li>
              ))}
            </ul>
          )}
        </div>
      </main>
    </div>
  );
}

function ProjectCard({
  project,
  now,
  query,
}: {
  project: ProjectSummary;
  now: number;
  query: string;
}) {
  const { overview } = project;
  const cost = projectCostLines(overview);
  const mark = projectMark(project);
  const actions = projectActions(project);
  const blocker = addBackBlocker(project);
  const content = (
    <>
      <div className="flex min-w-0 flex-1 flex-col gap-4">
        <span className="flex min-w-0 flex-wrap items-center gap-x-10 gap-y-4">
          <span className="min-w-0 truncate text-item-title leading-snug font-semibold text-text">
            {project.name}
          </span>
          <Mono className="text-meta text-text-muted">{project.slug}</Mono>
          {mark !== null ? (
            <span className="rounded-full bg-sunken px-8 py-2 text-meta leading-snug font-medium text-text-muted inset-ring inset-ring-border">
              {mark}
            </span>
          ) : null}
        </span>
        <span className="text-bar text-text-muted">
          {summaryLine(project, now)}
        </span>
        {showsRoot(project, query) ? (
          <Mono
            className="truncate text-meta text-text-muted"
            title={project.root}
          >
            {project.root}
          </Mono>
        ) : null}
        {blocker !== null ? (
          <span className="text-bar text-text-muted">
            <CodeText text={blocker} />
          </span>
        ) : null}
      </div>
      {overview ? (
        <>
          <div className="flex min-w-0 flex-col gap-6 md:contents">
            <ProjectAgentChips
              agents={overview.agents}
              className="w-full md:w-project-agents md:shrink-0"
            />
            <span className="flex shrink-0 items-center gap-6 text-code whitespace-nowrap md:w-project-state md:justify-end">
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
            {cost ? (
              <span className="flex shrink-0 flex-col md:items-end">
                <span className="text-item-title leading-snug font-semibold text-text tabular-nums">
                  {cost.total}
                </span>
                <span className="text-meta whitespace-nowrap text-text-muted">
                  {cost.recent}
                </span>
                <span className="text-meta whitespace-nowrap text-text-muted tabular-nums">
                  {cost.tokens}
                </span>
              </span>
            ) : null}
          </div>
          <ChevronRight
            aria-hidden
            size={16}
            className="hidden shrink-0 text-text-faint md:block"
          />
        </>
      ) : null}
    </>
  );
  const side =
    actions.remove || actions.addBack ? (
      <div className="flex shrink-0 items-start md:items-center">
        {actions.remove ? <RemoveProject project={project} /> : null}
        {actions.addBack ? <AddBackProject project={project} /> : null}
      </div>
    ) : null;
  if (project.available) {
    return (
      <div className={CARD}>
        <div className="flex items-stretch">
          <HrefLink
            href={projectPath(project.slug, "board")}
            className={cn(
              LINE,
              "min-w-0 flex-1 rounded-tl-l pt-16 pr-12 pb-12 pl-20 hover:bg-sunken",
              focusRing,
            )}
          >
            {content}
          </HrefLink>
          {side ? (
            <div className="flex items-start pt-14 pr-12 md:items-center md:pt-16 md:pb-12">
              {side}
            </div>
          ) : null}
        </div>
        <GitStatus
          slug={project.slug}
          className="mx-20 border-t border-border-subtle pt-12 pb-16"
        />
      </div>
    );
  }
  return (
    <div className={cn(ROW, "flex items-start gap-12")}>
      <div className={cn(LINE, "min-w-0 flex-1 opacity-60")}>{content}</div>
      {side}
    </div>
  );
}
