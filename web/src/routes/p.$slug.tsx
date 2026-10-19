import { createFileRoute, notFound } from "@tanstack/react-router";
import { projectQuery } from "../api/queries";
import { isProjectGone } from "../app/projectGone";
import { ProjectNotFound } from "../app/ProjectNotFound";
import { Shell } from "../app/Shell";
import { LiveProvider } from "../live/LiveProvider";
import { parseSlug } from "./params";

export const Route = createFileRoute("/p/$slug")({
  beforeLoad: ({ params }) => {
    if (parseSlug(params.slug) === null) {
      notFound({ throw: true });
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
        notFound({ throw: true });
      }
      throw error;
    }
  },
  component: ProjectLayout,
  notFoundComponent: ProjectNotFound,
});

function ProjectLayout() {
  const { slug } = Route.useParams();
  return (
    <LiveProvider slug={slug}>
      <Shell slug={slug} />
    </LiveProvider>
  );
}
