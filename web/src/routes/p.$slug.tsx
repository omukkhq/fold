import {
  createFileRoute,
  notFound,
  type NotFoundRouteProps,
} from "@tanstack/react-router";
import { projectQuery } from "../api/queries";
import { MissingPanel } from "../app/MissingPanel";
import {
  PROJECT_MISSING,
  isProjectGone,
  isProjectMissing,
} from "../app/projectGone";
import { ProjectNotFound } from "../app/ProjectNotFound";
import { Shell } from "../app/Shell";
import { LiveProvider } from "../live/LiveProvider";
import { parseSlug } from "./params";

export const Route = createFileRoute("/p/$slug")({
  beforeLoad: ({ params }) => {
    if (parseSlug(params.slug) === null) {
      notFound({ throw: true, data: PROJECT_MISSING });
    }
  },
  loader: async ({ context, params }) => {
    try {
      await context.queryClient.query({
        ...projectQuery(params.slug),
        staleTime: "static",
      });
    } catch (error) {
      if (isProjectGone(error)) {
        notFound({ throw: true, data: PROJECT_MISSING });
      }
      throw error;
    }
  },
  component: ProjectLayout,
  notFoundComponent: ProjectRouteNotFound,
});

function ProjectLayout() {
  const { slug } = Route.useParams();
  return (
    <LiveProvider slug={slug}>
      <Shell slug={slug} />
    </LiveProvider>
  );
}

/** A project fold does not know shows its state alone; an address inside an open project that matches no screen shows the page state in the shell's content area, where the router renders it. */
function ProjectRouteNotFound({ data }: NotFoundRouteProps) {
  const { slug } = Route.useParams();
  return isProjectMissing(data) ? (
    <ProjectNotFound />
  ) : (
    <MissingPanel slug={slug} missing={{ kind: "page" }} />
  );
}
