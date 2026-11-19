import { createFileRoute } from "@tanstack/react-router";
import { Forms } from "../components/Forms";
import { Functions } from "../components/Functions";
import { Hero } from "../components/Hero";
import { Language } from "../components/Language";
import { Engine, Evidence, Footer, Hud, InkFilters, Join, Sentence, Start } from "../components/Sections";
import {
  HOME_DESCRIPTION,
  HOME_TITLE,
  OG_IMAGE,
  SHARE_DESCRIPTION,
  SHARE_TITLE,
  SITE_NAME,
  SITE_URL,
  homeJsonLd,
  jsonLdString,
} from "../lib/seo";
import { Writing } from "../components/Writing";
import { usePlay } from "../lib/usePlay";

declare const __BUILD_DATE__: string;

export const Route = createFileRoute("/")({
  head: () => ({
    meta: [
      { title: HOME_TITLE },
      { name: "description", content: HOME_DESCRIPTION },
      { name: "robots", content: "index, follow, max-image-preview:large, max-snippet:-1, max-video-preview:-1" },
      { property: "og:type", content: "website" },
      { property: "og:site_name", content: SITE_NAME },
      { property: "og:locale", content: "en_US" },
      { property: "og:url", content: `${SITE_URL}/` },
      { property: "og:title", content: SHARE_TITLE },
      { property: "og:description", content: SHARE_DESCRIPTION },
      { property: "og:image", content: OG_IMAGE.url },
      { property: "og:image:secure_url", content: OG_IMAGE.url },
      { property: "og:image:type", content: "image/png" },
      { property: "og:image:width", content: String(OG_IMAGE.width) },
      { property: "og:image:height", content: String(OG_IMAGE.height) },
      { property: "og:image:alt", content: OG_IMAGE.alt },
      { name: "twitter:card", content: "summary_large_image" },
      { name: "twitter:title", content: SHARE_TITLE },
      { name: "twitter:description", content: SHARE_DESCRIPTION },
      { name: "twitter:image", content: OG_IMAGE.url },
      { name: "twitter:image:alt", content: OG_IMAGE.alt },
    ],
    links: [{ rel: "canonical", href: `${SITE_URL}/` }],
    scripts: [{ type: "application/ld+json", children: jsonLdString(homeJsonLd(__BUILD_DATE__)) }],
  }),
  component: Home,
});

function Home() {
  const { line, say, hoot } = usePlay();
  return (
    <>
      <InkFilters />
      <Hud />
      <main id="top">
        <Hero line={line} say={say} hoot={hoot} />
        <Sentence />
        <Language />
        <Forms />
        <Functions />
        <Writing />
        <Engine />
        <Evidence />
        <Start />
        <Join />
      </main>
      <Footer />
    </>
  );
}
