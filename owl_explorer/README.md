# Owl Explorer UI (React)

Owl Explorer is now implemented as a `React + TypeScript + Tailwind + shadcn-style` SPA.

## Stack

- React + TypeScript (Vite)
- Tailwind CSS
- shadcn-style UI primitives (`cn`, `cva`, Radix tabs/select)

## Run UI in dev mode

```bash
cd owl_explorer
npm install
npm run dev
```

## Build UI for Rust server

```bash
cd owl_explorer
npm install
npm run build
```

This outputs static assets into `owl_explorer/dist`.

## Run Owl Explorer backend

```bash
cargo run --bin owl-explorer
```

Then open `http://127.0.0.1:8787`.

- API endpoints remain unchanged under `/api/*`.
- Rust now serves `owl_explorer/dist` static assets with SPA fallback.
- If `dist` is missing, it falls back to `owl_explorer/index.html`.
