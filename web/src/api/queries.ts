import { queryOptions } from "@tanstack/react-query";
import { keys } from "./keys";
import { getProject, listProjects } from "./projects";
import { getSession } from "./session";

export const projectsQuery = () =>
  queryOptions({ queryKey: keys.projects(), queryFn: listProjects });

export const projectQuery = (slug: string) =>
  queryOptions({
    queryKey: keys.project(slug),
    queryFn: () => getProject(slug),
  });

export const sessionQuery = () =>
  queryOptions({ queryKey: keys.session(), queryFn: getSession });
