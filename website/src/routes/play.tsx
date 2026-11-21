import { createFileRoute } from "@tanstack/react-router";
import { Playground } from "../components/Playground";
import { OG_IMAGE, PLAY_DESCRIPTION, PLAY_TITLE, SITE_NAME, SITE_URL } from "../lib/seo";

export const Route = createFileRoute("/play")({
  head: () => ({
    meta: [
      { title: PLAY_TITLE },
      { name: "description", content: PLAY_DESCRIPTION },
      { name: "robots", content: "index, follow, max-image-preview:large, max-snippet:-1" },
      { property: "og:type", content: "website" },
      { property: "og:site_name", content: SITE_NAME },
      { property: "og:locale", content: "en_US" },
      { property: "og:url", content: `${SITE_URL}/play` },
      { property: "og:title", content: PLAY_TITLE },
      { property: "og:description", content: PLAY_DESCRIPTION },
      { property: "og:image", content: OG_IMAGE.url },
      { property: "og:image:width", content: String(OG_IMAGE.width) },
      { property: "og:image:height", content: String(OG_IMAGE.height) },
      { property: "og:image:alt", content: OG_IMAGE.alt },
      { name: "twitter:card", content: "summary_large_image" },
      { name: "twitter:title", content: PLAY_TITLE },
      { name: "twitter:description", content: PLAY_DESCRIPTION },
      { name: "twitter:image", content: OG_IMAGE.url },
    ],
    links: [{ rel: "canonical", href: `${SITE_URL}/play` }],
  }),
  component: Playground,
});
