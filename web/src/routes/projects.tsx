import { createFileRoute } from "@tanstack/react-router";
import { projectsQuery } from "../api/queries";
import { ProjectsScreen } from "../features/projects/ProjectsScreen";

export const Route = createFileRoute("/projects")({
  loader: ({ context }) => context.queryClient.query(projectsQuery()),
  component: ProjectsScreen,
});
