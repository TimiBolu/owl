import { createFileRoute } from "@tanstack/react-router";

// The page was first published here. It lives at /try now, and anyone who kept the old address is sent on.
export const Route = createFileRoute("/play")({
  server: {
    handlers: {
      GET: () => new Response(null, { status: 308, headers: { location: "/try", "cache-control": "public, max-age=86400" } }),
    },
  },
  component: () => null,
});
