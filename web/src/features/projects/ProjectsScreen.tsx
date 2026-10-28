import { useSuspenseQuery } from "@tanstack/react-query";
import { FolderGit2, Search, SearchX } from "lucide-react";
import { useState } from "react";
import type { ProjectSummary } from "@/api/projects";
import { CodeText } from "@/components/code/CodeText";
import { useNow } from "@/live/useNow";
import { projectsQuery } from "@/api/queries";
import { HrefLink } from "@/app/HrefLink";
import { AvatarMenu } from "@/app/AvatarMenu";
import { Chip } from "@/components/badge/Chip";
import { Mono } from "@/components/mono/Mono";
import { Tooltip } from "@/components/overlay/Tooltip";
import { Wordmark } from "@/components/Wordmark";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { allProjectsCost, projectCostLines } from "../cost/cost";
import { GitStatus } from "../git/GitStatus";
import { lastChangeLine, summaryLine } from "./counts";
import { AddBackProject } from "./AddBackProject";
import { ProjectAgents } from "./ProjectAgents";
import {
  addBackBlocker,
  projectActions,
  projectMark,
  shortPath,
  showsRoot,
  shownProjects,
} from "./projectsHome";
import { RemoveProject } from "./RemoveProject";
import { projectPath } from "@/routes/paths";

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
      <header className="flex h-bar-compact items-center justify-between bg-surface px-16 xl:h-bar xl:px-20">
        <Wordmark />
        <AvatarMenu />
      </header>
      <main className="mx-auto flex w-full flex-col gap-18 px-16 pt-24 pb-40 md:px-24 md:pt-32 xl:w-content">
        <div className="flex flex-wrap items-end justify-between gap-x-24 gap-y-12">
          <div className="flex flex-col gap-4">
            <h1 className="text-28 leading-tight font-semibold text-text">
              Projects
            </h1>
            <p className="text-15 text-text-2">
              Every project this fold daemon serves on this machine.
            </p>
          </div>
          <p className="flex flex-wrap items-center gap-8 text-14 text-text-2">
            Add a project:
            <Mono className="rounded-s bg-surface px-8 py-4 text-text">
              fld init &lt;dir&gt;
            </Mono>
          </p>
        </div>
        {projects.length > 0 ? (
          <div className="flex flex-wrap items-center gap-x-24 gap-y-12">
            <label className="flex w-full min-w-0 items-center sm:w-auto sm:flex-1 gap-10 rounded-m bg-surface px-14 py-10 has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-2 has-[input:focus-visible]:outline-accent">
              <Search
                aria-hidden="true"
                size={16}
                className="shrink-0 text-text-2"
              />
              <input
                type="search"
                aria-label="Search projects"
                placeholder="Search by name, slug or folder"
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                }}
                className="min-w-0 flex-1 bg-transparent text-15 leading-tight text-text outline-none placeholder:text-text-2"
              />
            </label>
            <p className="text-14 text-text-2 tabular-nums">
              All projects{" "}
              <span className="text-15 font-semibold text-text">
                {total.cost}
              </span>{" "}
              · {total.tokens}
            </p>
          </div>
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
  const main = (
    <div className="flex min-w-0 flex-col gap-4">
      <span className="flex min-w-0 flex-wrap items-baseline gap-x-10 gap-y-4">
        <span className="min-w-0 font-title text-18 leading-tight font-semibold text-text group-hover:underline group-hover:underline-offset-4">
          {project.name}
        </span>
        <span className="text-13 text-text-2">{project.slug}</span>
        {mark !== null ? (
          <Chip className="bg-surface text-text-2">{mark}</Chip>
        ) : null}
      </span>
      <span
        className={cn(
          "text-14",
          overview ? "text-text tabular-nums" : "text-text-2",
        )}
      >
        {summaryLine(project, now)}
      </span>
      {overview ? (
        <span className="flex flex-wrap items-center gap-x-6 gap-y-2 text-14">
          <ProjectAgents agents={overview.agents} />
          <span className="text-13 text-text-2">
            · {lastChangeLine(overview, now)}
          </span>
        </span>
      ) : null}
      {showsRoot(project, query) ? <RootPath root={project.root} /> : null}
      {blocker !== null ? (
        <span className="text-14 text-text-2">
          <CodeText text={blocker} />
        </span>
      ) : null}
    </div>
  );
  const side =
    actions.remove || actions.addBack ? (
      <div className="flex shrink-0 items-start">
        {actions.remove ? <RemoveProject project={project} /> : null}
        {actions.addBack ? <AddBackProject project={project} /> : null}
      </div>
    ) : null;
  if (project.available) {
    return (
      <div className="flex flex-col overflow-hidden rounded-l bg-surface">
        <div className="flex items-start gap-8 pt-16 pr-12 pb-14 pl-20 max-md:pl-16">
          <HrefLink
            href={projectPath(project.slug, "board")}
            className={cn(
              "group grid min-w-0 flex-1 grid-cols-1 gap-x-32 gap-y-8 rounded-m md:grid-cols-[minmax(0,1fr)_auto]",
              focusRing,
            )}
          >
            {main}
            {cost ? (
              <span className="flex flex-wrap items-baseline gap-x-10 gap-y-2 md:flex-col md:items-end md:gap-2 md:pt-2 md:text-right">
                <span className="font-title text-18 leading-tight font-semibold text-text tabular-nums">
                  {cost.total}
                </span>
                <span className="text-13 whitespace-nowrap text-text-2 tabular-nums">
                  {cost.recent}
                </span>
                <span className="text-13 whitespace-nowrap text-text-2 tabular-nums">
                  {cost.tokens}
                </span>
              </span>
            ) : null}
          </HrefLink>
          {side}
        </div>
        <GitStatus
          slug={project.slug}
          className="bg-sunken/55 px-20 pt-10 pb-12 max-md:px-16"
        />
      </div>
    );
  }
  return (
    <div className="flex items-start gap-8 rounded-l bg-sunken pt-16 pr-12 pb-14 pl-20 max-md:pl-16">
      <div className="min-w-0 flex-1">{main}</div>
      {side}
    </div>
  );
}

/** The project's folder, shortened to its last folders; the whole path is in the tooltip, which focus opens too, and in the Remove dialog. */
function RootPath({ root }: { root: string }) {
  const short = shortPath(root);
  if (short === root) {
    return <Mono className="truncate text-13 text-text-2">{root}</Mono>;
  }
  return (
    <Tooltip content={root}>
      <Mono
        tabIndex={0}
        className={cn("self-start truncate text-13 text-text-2", focusRing)}
      >
        <span aria-hidden="true">{short}</span>
        <span className="sr-only">{root}</span>
      </Mono>
    </Tooltip>
  );
}
