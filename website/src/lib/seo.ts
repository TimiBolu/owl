/** The one host search engines may index. www redirects here; pubp preview hosts are noindexed. */
export const CANONICAL_HOST = "owldb.tech";
export const SITE_URL = `https://${CANONICAL_HOST}`;

export const SITE_NAME = "Owl";
export const GITHUB_URL = "https://github.com/TimiBolu/owl";

// What a search result shows: a title under 60 characters and a description under 160.
export const HOME_TITLE = "Owl: an object-graph database in Rust, queried without joins";
export const HOME_DESCRIPTION =
  "Owl is an object-graph database written in Rust. Follow references instead of writing joins, with one query language, OQL, for reads, writes and rules.";

// What a shared link shows, in WhatsApp, Slack, X and the rest: shorter, and led by the idea.
export const SHARE_TITLE = "Owl: your data is already a graph. Query it that way.";
export const SHARE_DESCRIPTION =
  "An object-graph database written in Rust. Follow references instead of writing joins.";

export const OG_IMAGE = {
  url: `${SITE_URL}/og-image.png`,
  width: 1200,
  height: 630,
  alt: "Owl, an object-graph database. Your data is already a graph. Query it that way.",
};

/** The sections of the page, in order: used by the footer links and the structured data. */
export const SECTIONS = [
  ["language", "The language", "A seven-chapter tour of OQL, from a schema to recursion and row positions."],
  ["forms", "Chain and block forms", "The two equivalent ways to write any OQL query, and what a block adds."],
  ["functions", "Functions", "Naming a measure, a rule, a test or a path, and keeping it in the database."],
  ["writing", "Writing data", "Insert, update, upsert and delete, the schema's rules, and transactions."],
  ["engine", "The engine", "How Owl stores objects, records, writes and indexes."],
  ["evidence", "Evidence", "How every answer is checked against SQLite before anything is timed."],
  ["start", "Run it", "Three commands from nothing to a first answer."],
] as const;

const SITE_ID = `${SITE_URL}/#website`;
const PAGE_ID = `${SITE_URL}/#webpage`;
const APP_ID = `${SITE_URL}/#software`;

/** schema.org graph for the home page. Everything here mirrors what the page itself says. */
export function homeJsonLd(dateModified: string) {
  return {
    "@context": "https://schema.org",
    "@graph": [
      {
        "@type": "WebSite",
        "@id": SITE_ID,
        url: `${SITE_URL}/`,
        name: SITE_NAME,
        alternateName: ["Owl database", "OwlDB"],
        description: HOME_DESCRIPTION,
        inLanguage: "en",
      },
      {
        "@type": "WebPage",
        "@id": PAGE_ID,
        url: `${SITE_URL}/`,
        name: HOME_TITLE,
        description: HOME_DESCRIPTION,
        isPartOf: { "@id": SITE_ID },
        about: { "@id": APP_ID },
        primaryImageOfPage: { "@type": "ImageObject", url: OG_IMAGE.url, width: OG_IMAGE.width, height: OG_IMAGE.height },
        inLanguage: "en",
        dateModified,
        hasPart: SECTIONS.map(([id, name, description]) => ({
          "@type": "WebPageElement",
          name,
          description,
          url: `${SITE_URL}/#${id}`,
        })),
      },
      {
        "@type": ["SoftwareApplication", "SoftwareSourceCode"],
        "@id": APP_ID,
        name: SITE_NAME,
        alternateName: ["Owl database", "OwlDB"],
        description: HOME_DESCRIPTION,
        url: `${SITE_URL}/`,
        image: OG_IMAGE.url,
        applicationCategory: "DeveloperApplication",
        applicationSubCategory: "Database",
        codeRepository: GITHUB_URL,
        programmingLanguage: "Rust",
        creativeWorkStatus: "Under active development, not yet for production data",
        keywords: "object-graph database, OQL, query language, references, Rust, embedded database, vector search",
        featureList: [
          "OQL, a query language that follows references instead of joining",
          "Chain and block forms of the same query",
          "Functions written in OQL and stored in the database",
          "Schema rules: unique, check, references and cascade",
          "Transactions and atomic single statements",
          "A storage engine written in Rust with a write-ahead log, checkpoints and recovery",
          "Ordered, unique, composite, reverse-reference and vector indexes",
        ],
      },
    ],
  };
}

/** Safe for inlining inside <script type="application/ld+json">. */
export const jsonLdString = (data: unknown) => JSON.stringify(data).replace(/</g, "\\u003c");
