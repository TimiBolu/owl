import { createFileRoute } from "@tanstack/react-router";
import { requestHost } from "../lib/host";
import { CANONICAL_HOST, SITE_URL } from "../lib/seo";

export const Route = createFileRoute("/robots.txt")({
  server: {
    handlers: {
      GET: ({ request }) => {
        const body =
          requestHost(request.headers) === CANONICAL_HOST
            ? ["User-agent: *", "Allow: /", "Disallow: /health-check", "", `Sitemap: ${SITE_URL}/sitemap.xml`, ""]
            : [
                `# Preview host. The canonical site is ${SITE_URL}/`,
                "# Crawling stays allowed so crawlers can see the X-Robots-Tag: noindex header on every page.",
                "User-agent: *",
                "Allow: /",
                "",
              ];
        return new Response(body.join("\n"), {
          headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "public, max-age=3600" },
        });
      },
    },
  },
  component: () => null,
});
