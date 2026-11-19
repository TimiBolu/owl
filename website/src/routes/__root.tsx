/// <reference types="vite/client" />
import { HeadContent, Outlet, Scripts, createRootRoute } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { SITE_NAME } from "../lib/seo";
import styles from "../styles.css?url";

// Runs before first paint. Marks <html class="js"> so styles that hide content until it is
// scrolled into view only apply when JavaScript is running.
const BOOT_SCRIPT = `document.documentElement.classList.add("js")`;

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1" },
      { name: "application-name", content: SITE_NAME },
      { name: "theme-color", content: "#e9e0c9" },
    ],
    links: [
      { rel: "stylesheet", href: styles },
      { rel: "icon", href: "/favicon.svg", type: "image/svg+xml" },
    ],
  }),
  component: RootComponent,
  notFoundComponent: NotFound,
});

function NotFound() {
  return (
    <main className="lost">
      <title>Page not found — Owl</title>
      <meta name="robots" content="noindex, follow" />
      <h1 className="cut">
        <span className="cut-line l1">Nothing</span>
        <span className="cut-line l2">points here.</span>
      </h1>
      <p className="lede">This page does not exist.</p>
      <div className="cta">
        <a className="btn solid" href="/">
          Back to Owl
        </a>
      </div>
    </main>
  );
}

function RootComponent() {
  return (
    <RootDocument>
      <Outlet />
    </RootDocument>
  );
}

function RootDocument({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: BOOT_SCRIPT }} />
        <HeadContent />
      </head>
      <body>
        {children}
        <Scripts />
      </body>
    </html>
  );
}
