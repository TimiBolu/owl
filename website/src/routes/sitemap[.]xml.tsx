import { createFileRoute } from "@tanstack/react-router";
import { OG_IMAGE, SITE_URL } from "../lib/seo";

declare const __BUILD_DATE__: string;

export const Route = createFileRoute("/sitemap.xml")({
  server: {
    handlers: {
      GET: () =>
        new Response(
          `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
  <url>
    <loc>${SITE_URL}/</loc>
    <lastmod>${__BUILD_DATE__}</lastmod>
    <changefreq>weekly</changefreq>
    <priority>1.0</priority>
    <image:image><image:loc>${OG_IMAGE.url}</image:loc></image:image>
  </url>
  <url>
    <loc>${SITE_URL}/try</loc>
    <lastmod>${__BUILD_DATE__}</lastmod>
    <changefreq>weekly</changefreq>
    <priority>0.8</priority>
  </url>
</urlset>
`,
          { headers: { "content-type": "application/xml; charset=utf-8", "cache-control": "public, max-age=3600" } },
        ),
    },
  },
  component: () => null,
});
