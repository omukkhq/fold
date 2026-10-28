import { useQuery } from "@tanstack/react-query";
import { createFileRoute, notFound } from "@tanstack/react-router";
import { MissingPanel } from "../app/MissingPanel";
import { sessionQuery } from "../api/queries";
import { RequestDetailScreen } from "../features/requests/RequestDetailScreen";
import { parseRequestParam } from "./params";

export const Route = createFileRoute("/p/$slug/requests_/$id")({
  beforeLoad: ({ params }) => {
    if (parseRequestParam(params.id) === null) {
      notFound({ throw: true });
    }
  },
  notFoundComponent: RequestNotFound,
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

function RequestNotFound() {
  const { slug, id } = Route.useParams();
  return <MissingPanel slug={slug} missing={{ kind: "request", id: id }} />;
}
