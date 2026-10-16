import { createFileRoute, notFound } from "@tanstack/react-router";
import { isApiError } from "../api/errors";
import { reviewBatchQuery } from "../api/queries";
import { BatchPending, BatchScreen } from "../features/batches/BatchScreen";
import { isBatchId } from "../features/workers/search";

export const Route = createFileRoute("/p/$slug/batches/$id")({
  beforeLoad: ({ params }) => {
    if (!isBatchId(params.id)) {
      notFound({ throw: true });
    }
  },
  loader: async ({ context, params }) => {
    try {
      await context.queryClient.query({
        ...reviewBatchQuery(params.slug, params.id),
        staleTime: "static",
      });
    } catch (error) {
      if (isApiError(error, "review_batch_not_found")) {
        notFound({ throw: true });
      }
      throw error;
    }
  },
  pendingComponent: BatchPending,
  component: BatchRoute,
});

function BatchRoute() {
  const { slug, id } = Route.useParams();
  return <BatchScreen slug={slug} id={id} />;
}
