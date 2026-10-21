import { useRouter, type ErrorComponentProps } from "@tanstack/react-router";
import { ErrorBanner } from "../components/states/ErrorBanner";
import { Skeleton } from "../components/states/Skeleton";
import { projectProblem } from "./projectGone";
import { ProjectNotFound, ProjectRemoved } from "./ProjectNotFound";
import { ProjectUnavailable } from "./ProjectUnavailable";

export function RoutePending() {
  return (
    <div className="p-24">
      <Skeleton />
    </div>
  );
}

/** A screen whose project fold does not know shows the not-found state instead of a retry, as no retry brings the project back; one whose project the daemon cannot open now says why. */
export function RouteError({ error }: Pick<ErrorComponentProps, "error">) {
  const router = useRouter();
  const problem = projectProblem(error);
  if (problem?.kind === "gone") {
    return <ProjectNotFound />;
  }
  if (problem?.kind === "removed") {
    return <ProjectRemoved hint={problem.hint} />;
  }
  if (problem) {
    return (
      <ProjectUnavailable
        problem={problem}
        onRetry={() => void router.invalidate()}
      />
    );
  }
  return (
    <ErrorBanner
      title="This screen could not load."
      message="The data did not arrive."
      // Resetting the boundary alone keeps the failed loader result; invalidating re-runs the loaders and resets it.
      onRetry={() => void router.invalidate()}
    />
  );
}
