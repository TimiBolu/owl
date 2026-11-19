# Owl website

The landing site for Owl. Built with TanStack Start, rendered on the server by Nitro, and
deployed on pubp. There is no backend.

## Layout

```text
../.pubp/github.json   pubp CI: one service, deploys `main` to prod
../.pubpignore         keeps the database's build output and data out of pubp uploads
Dockerfile             node:22-alpine, runs `node .output/server/index.mjs` on :3000
middleware/            www → apex 308, noindex on every non-canonical host
src/routes/            index, health-check, robots.txt, sitemap.xml (and the 404 in __root)
src/components/        Hero (owl, draggable graph), Language (tabs), Sections (the rest)
src/lib/usePlay.ts     everything that moves after load; the page is complete without it
src/lib/seo.ts         titles, description, and CANONICAL_HOST
```

## Local

```bash
npm install
npm run dev                          # http://localhost:5181
npm run build && PORT=3000 npm start
```

## pubp

| | |
|---|---|
| Tenant / app / environment | `owl` / `website` / `prod` |
| Service | `public` (docker, port 3000, healthcheck `/health-check`) |
| Address | https://owldb.tech (canonical), `www.` redirects; https://prod.website.owl.publish-preview.com (noindex) |

Publish from the repository root:

```bash
pubp publish public --tenant owl --app website --env prod --path ./website --remote --strategy docker_server
```

or with the rules CI uses:

```bash
pubp ci github run --branch main --changed-files website/package.json
```

`owldb.tech` and `www.owldb.tech` are bound to the `public` service:
`pubp domain list --tenant owl --app website --env prod`. At the DNS host, the root has an
A record to `109.123.246.2` and `www` is a CNAME to `website-edge.publish-preview.com`.
Keep the `_pubp-challenge` TXT records.

`CANONICAL_HOST` in `src/lib/seo.ts` is the only host search engines may index. Every other
host answers with `X-Robots-Tag: noindex`.
