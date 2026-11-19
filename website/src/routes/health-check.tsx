import { createFileRoute } from "@tanstack/react-router";

// pubp `public` service healthcheck (GET /health-check → 200).
export const Route = createFileRoute("/health-check")({
  server: {
    handlers: {
      GET: () =>
        new Response("ok", {
          status: 200,
          headers: {
            "content-type": "text/plain; charset=utf-8",
            "cache-control": "no-store",
            "x-robots-tag": "noindex, nofollow, noarchive",
          },
        }),
    },
  },
  component: () => null,
});
