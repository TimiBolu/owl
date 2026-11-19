import { defineEventHandler, redirect } from "h3";
import { CANONICAL_HOST, SITE_URL } from "../src/lib/seo";
import { requestHost } from "../src/lib/host";

// Hosts that permanently redirect to the canonical origin.
const ALTERNATE_HOSTS = new Set([`www.${CANONICAL_HOST}`]);

export default defineEventHandler((event) => {
  const host = requestHost(event.req.headers);
  const { pathname, search } = event.url;

  if (pathname === "/health-check") return; // the pubp probe must answer on any host

  if (ALTERNATE_HOSTS.has(host)) {
    return redirect(`${SITE_URL}${pathname}${search}`, 308);
  }

  // Any other host (the pubp preview address) stays reachable but is never indexed.
  if (host !== CANONICAL_HOST) {
    event.res.headers.set("x-robots-tag", "noindex, nofollow");
  }
});
