import { createFileRoute } from "@tanstack/react-router";
import { Playground } from "../components/Playground";
import { OG_IMAGE, SITE_NAME, SITE_URL, TRY_DESCRIPTION, TRY_TITLE } from "../lib/seo";

export const Route = createFileRoute("/try")({
  head: () => ({
    meta: [
      { title: TRY_TITLE },
      { name: "description", content: TRY_DESCRIPTION },
      { name: "robots", content: "index, follow, max-image-preview:large, max-snippet:-1" },
      { property: "og:type", content: "website" },
      { property: "og:site_name", content: SITE_NAME },
      { property: "og:locale", content: "en_US" },
      { property: "og:url", content: `${SITE_URL}/try` },
      { property: "og:title", content: TRY_TITLE },
      { property: "og:description", content: TRY_DESCRIPTION },
      { property: "og:image", content: OG_IMAGE.url },
      { property: "og:image:width", content: String(OG_IMAGE.width) },
      { property: "og:image:height", content: String(OG_IMAGE.height) },
      { property: "og:image:alt", content: OG_IMAGE.alt },
      { name: "twitter:card", content: "summary_large_image" },
      { name: "twitter:title", content: TRY_TITLE },
      { name: "twitter:description", content: TRY_DESCRIPTION },
      { name: "twitter:image", content: OG_IMAGE.url },
    ],
    links: [{ rel: "canonical", href: `${SITE_URL}/try` }],
  }),
  component: Playground,
});
