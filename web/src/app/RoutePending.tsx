export function RoutePending() {
  return (
    <div
      role="status"
      aria-label="Loading"
      className="flex flex-col gap-12 p-24"
    >
      <div className="h-24 w-40 animate-pulse rounded-m bg-sunken" />
      <div className="h-24 w-32 animate-pulse rounded-m bg-sunken" />
      <div className="h-24 w-28 animate-pulse rounded-m bg-sunken" />
    </div>
  );
}
