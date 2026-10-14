import { createFileRoute } from "@tanstack/react-router";
import { BoardScreen } from "../features/board/BoardScreen";
import { boardSearchSchema, cleanSearch } from "../features/board/search";

export const Route = createFileRoute("/p/$slug/board")({
  validateSearch: boardSearchSchema,
  component: BoardRoute,
});

function BoardRoute() {
  const { slug } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <BoardScreen
      slug={slug}
      search={search}
      onSearch={(next) => {
        void navigate({ search: cleanSearch(next), replace: true });
      }}
    />
  );
}
