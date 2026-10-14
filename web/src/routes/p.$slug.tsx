import { createFileRoute, notFound } from "@tanstack/react-router";
import { hasErrorCode } from "../api/errors";
import { projectQuery } from "../api/queries";
import { Shell } from "../app/Shell";
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
      if (hasErrorCode(error, "not_found")) {
        notFound({ throw: true });
      }
      throw error;
    }
  },
  component: ProjectLayout,
});

function ProjectLayout() {
  const { slug } = Route.useParams();
  return <Shell slug={slug} />;
}
