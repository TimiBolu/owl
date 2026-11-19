import { createFileRoute } from "@tanstack/react-router";
import { SITE_URL } from "../lib/seo";

declare const __BUILD_DATE__: string;

export const Route = createFileRoute("/sitemap.xml")({
  server: {
    handlers: {
      GET: () =>
        new Response(
          `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>${SITE_URL}/</loc>
    <lastmod>${__BUILD_DATE__}</lastmod>
  </url>
</urlset>
`,
          { headers: { "content-type": "application/xml; charset=utf-8", "cache-control": "public, max-age=3600" } },
        ),
    },
  },
  component: () => null,
});
