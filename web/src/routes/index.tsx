import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/")({
  beforeLoad: () => {
    // Untyped href: the projects route belongs to its own screen.
    redirect({ href: "/projects", replace: true, throw: true });
  },
});
