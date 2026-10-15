import { useQuery } from "@tanstack/react-query";
import { createFileRoute, notFound } from "@tanstack/react-router";
import { sessionQuery } from "../api/queries";
import { RequestDetailScreen } from "../features/requests/RequestDetailScreen";
import { parseRequestParam } from "./params";

export const Route = createFileRoute("/p/$slug/requests_/$id")({
  beforeLoad: ({ params }) => {
    if (parseRequestParam(params.id) === null) {
      notFound({ throw: true });
    }
  },
  component: RequestDetailRoute,
});

function RequestDetailRoute() {
  const { slug, id } = Route.useParams();
  const session = useQuery(sessionQuery());
  return (
    <RequestDetailScreen
      key={id}
      slug={slug}
      id={id}
      human={session.data?.human ?? null}
    />
  );
}
