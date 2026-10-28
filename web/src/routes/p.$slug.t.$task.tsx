import { createFileRoute, notFound } from "@tanstack/react-router";
import { MissingPanel } from "../app/MissingPanel";
import { BoardScreen } from "../features/board/BoardScreen";
import { boardSearchSchema, cleanSearch } from "../features/board/search";
import { TaskDialog } from "../features/task/TaskDialog";
import { parseTaskParam } from "./params";

/** The task dialog over the board. It takes the board's filters as its own search, so closing returns to the same board. */
export const Route = createFileRoute("/p/$slug/t/$task")({
  validateSearch: boardSearchSchema,
  beforeLoad: ({ params }) => {
    if (parseTaskParam(params.task) === null) {
      notFound({ throw: true });
    }
  },
  notFoundComponent: TaskNotFound,
  component: TaskRoute,
});

function TaskRoute() {
  const { slug, task } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <>
      <BoardScreen
        slug={slug}
        search={search}
        onSearch={(next) => {
          void navigate({ search: cleanSearch(next), replace: true });
        }}
      />
      <TaskDialog
        key={task}
        slug={slug}
        task={task}
        onClose={() => {
          void navigate({
            to: "/p/$slug/board",
            params: { slug },
            search: cleanSearch(search),
          });
        }}
      />
    </>
  );
}

function TaskNotFound() {
  const { slug, task } = Route.useParams();
  return <MissingPanel slug={slug} missing={{ kind: "task", id: task }} />;
}
