import { useRouter } from "@tanstack/react-router";

export function RouteError() {
  const router = useRouter();
  return (
    <div className="p-24">
      <div
        role="alert"
        className="flex items-center justify-between gap-12 rounded-m border border-danger bg-danger-soft px-12 py-10 text-body text-text"
      >
        <span>This screen could not load.</span>
        <button
          type="button"
          onClick={() => {
            // Resetting the boundary alone keeps the failed loader result; invalidating re-runs the loaders and resets it.
            void router.invalidate();
          }}
          className="rounded-m border border-border bg-surface px-10 py-5 font-medium text-text outline-primary hover:bg-sunken focus-visible:outline-2"
        >
          Retry
        </button>
      </div>
    </div>
  );
}
