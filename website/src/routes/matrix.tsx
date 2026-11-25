import { createFileRoute } from "@tanstack/react-router";
import { Matrix } from "../components/Matrix";
import { MATRIX_DESCRIPTION, MATRIX_TITLE, OG_IMAGE, SITE_NAME, SITE_URL } from "../lib/seo";

export const Route = createFileRoute("/matrix")({
  head: () => ({
    meta: [
      { title: MATRIX_TITLE },
      { name: "description", content: MATRIX_DESCRIPTION },
      { name: "robots", content: "index, follow, max-image-preview:large, max-snippet:-1" },
      { property: "og:type", content: "website" },
      { property: "og:site_name", content: SITE_NAME },
      { property: "og:locale", content: "en_US" },
      { property: "og:url", content: `${SITE_URL}/matrix` },
      { property: "og:title", content: MATRIX_TITLE },
      { property: "og:description", content: MATRIX_DESCRIPTION },
      { property: "og:image", content: OG_IMAGE.url },
      { property: "og:image:width", content: String(OG_IMAGE.width) },
      { property: "og:image:height", content: String(OG_IMAGE.height) },
      { property: "og:image:alt", content: OG_IMAGE.alt },
      { name: "twitter:card", content: "summary_large_image" },
      { name: "twitter:title", content: MATRIX_TITLE },
      { name: "twitter:description", content: MATRIX_DESCRIPTION },
      { name: "twitter:image", content: OG_IMAGE.url },
    ],
    links: [{ rel: "canonical", href: `${SITE_URL}/matrix` }],
  }),
  component: Matrix,
});
