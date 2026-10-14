import { useRouter } from "@tanstack/react-router";
import { ErrorBanner } from "../components/states/ErrorBanner";
import { Skeleton } from "../components/states/Skeleton";

export function RoutePending() {
  return (
    <div className="p-24">
      <Skeleton />
    </div>
  );
}

export function RouteError() {
  const router = useRouter();
  return (
    <ErrorBanner
      title="This screen could not load."
      message="The data did not arrive."
      // Resetting the boundary alone keeps the failed loader result; invalidating re-runs the loaders and resets it.
      onRetry={() => void router.invalidate()}
    />
  );
}
