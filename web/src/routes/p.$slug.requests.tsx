import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { sessionQuery } from "../api/queries";
import { parseListSearch } from "../features/requests/filters";
import { RequestsScreen } from "../features/requests/RequestsScreen";

export const Route = createFileRoute("/p/$slug/requests")({
  validateSearch: parseListSearch,
  component: RequestsRoute,
});

function RequestsRoute() {
  const { slug } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const session = useQuery(sessionQuery());
  return (
    <RequestsScreen
      slug={slug}
      search={search}
      human={session.data?.human ?? null}
      onSearch={(next) => {
        void navigate({ search: parseListSearch({ ...next }), replace: true });
      }}
    />
  );
}
