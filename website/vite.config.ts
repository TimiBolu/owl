import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import viteReact from "@vitejs/plugin-react";
import { nitro } from "nitro/vite";
import { defineConfig } from "vite";

// Rendered on the server by TanStack Start and served by Nitro's Node server
// (.output/server/index.mjs) inside the pubp `public` container. There is no backend.
const SECURITY_HEADERS = {
  "strict-transport-security": "max-age=31536000",
  "x-content-type-options": "nosniff",
  "referrer-policy": "strict-origin-when-cross-origin",
  "x-frame-options": "SAMEORIGIN",
  "permissions-policy": "camera=(), microphone=(), geolocation=(), payment=()",
};

export default defineConfig({
  define: { __BUILD_DATE__: JSON.stringify(new Date().toISOString().slice(0, 10)) },
  plugins: [
    nitro({
      serverDir: ".",
      // The engine is 6.7 MB as built and under 2 MB compressed.
      compressPublicAssets: true,
      routeRules: {
        // Pages: always revalidate, so a deploy shows up at once.
        "/**": { headers: { ...SECURITY_HEADERS, "cache-control": "no-cache" } },
        // Unhashed public files: cache for a day.
        ...Object.fromEntries(
          ["/og-image.png", "/logo.png", "/favicon.ico", "/favicon.svg", "/apple-touch-icon.png", "/icon-192.png", "/icon-512.png", "/site.webmanifest", "/llms.txt"].map(
            (path) => [path, { headers: { ...SECURITY_HEADERS, "cache-control": "public, max-age=86400" } }],
          ),
        ),
        "/assets/**": { headers: { ...SECURITY_HEADERS, "cache-control": "public, max-age=31536000, immutable" } },
        // The engine for /play: large and unhashed, so it is kept for an hour and checked after.
        "/owl/**": { headers: { ...SECURITY_HEADERS, "cache-control": "public, max-age=3600, must-revalidate" } },
        "/health-check": { headers: { "cache-control": "no-store", "x-robots-tag": "noindex, nofollow, noarchive" } },
      },
    }),
    tanstackStart(),
    viteReact(),
  ],
  server: { port: 5181, host: true },
});
