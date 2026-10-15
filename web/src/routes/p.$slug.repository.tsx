import { createFileRoute } from "@tanstack/react-router";
import { RepositoryScreen } from "../features/repository/RepositoryScreen";
import { parseRepositorySearch } from "../features/repository/search";

export const Route = createFileRoute("/p/$slug/repository")({
  validateSearch: parseRepositorySearch,
  component: RepositoryRoute,
});

function RepositoryRoute() {
  const { slug } = Route.useParams();
  const { revision, path } = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <RepositoryScreen
      key={`${revision ?? ""}:${path ?? ""}`}
      slug={slug}
      revision={revision}
      path={path}
      onSelectRevision={(id) => {
        void navigate({ search: { revision: id } });
      }}
    />
  );
}
