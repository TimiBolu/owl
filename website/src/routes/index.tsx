import { createFileRoute } from "@tanstack/react-router";
import { Forms } from "../components/Forms";
import { Functions } from "../components/Functions";
import { Hero } from "../components/Hero";
import { Language } from "../components/Language";
import { Engine, Evidence, Footer, Hud, InkFilters, Sentence, Start } from "../components/Sections";
import { HOME_DESCRIPTION, HOME_TITLE, SITE_NAME, SITE_URL } from "../lib/seo";
import { Writing } from "../components/Writing";
import { usePlay } from "../lib/usePlay";

export const Route = createFileRoute("/")({
  head: () => ({
    meta: [
      { title: HOME_TITLE },
      { name: "description", content: HOME_DESCRIPTION },
      { property: "og:type", content: "website" },
      { property: "og:site_name", content: SITE_NAME },
      { property: "og:url", content: `${SITE_URL}/` },
      { property: "og:title", content: HOME_TITLE },
      { property: "og:description", content: HOME_DESCRIPTION },
      { name: "twitter:card", content: "summary" },
      { name: "twitter:title", content: HOME_TITLE },
      { name: "twitter:description", content: HOME_DESCRIPTION },
    ],
    links: [{ rel: "canonical", href: `${SITE_URL}/` }],
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
      </main>
      <Footer />
    </>
  );
}
