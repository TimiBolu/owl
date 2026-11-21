// Builds the engine for the browser and puts it where the site serves it:
//   npm run wasm
// Needs a Rust toolchain with the wasm32-unknown-unknown target, and wasm-pack.
// The files it writes into public/owl are committed, because the site is built
// in a container with no Rust in it. Run this again when the engine changes.
import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const site = join(dirname(fileURLToPath(import.meta.url)), "..");
const crate = join(site, "..", "apps", "owl-wasm");
const built = join(crate, "pkg-site");
const served = join(site, "public", "owl");

execFileSync("wasm-pack", ["build", crate, "--target", "web", "--release", "--out-dir", built], { stdio: "inherit" });

// Only what the page loads: the module, the engine, and the few lines of
// JavaScript the engine calls for the browser's files.
for (const name of readdirSync(served)) if (name !== "worker.js") rmSync(join(served, name), { recursive: true });
mkdirSync(served, { recursive: true });
cpSync(join(built, "owl_wasm.js"), join(served, "owl_wasm.js"));
cpSync(join(built, "owl_wasm_bg.wasm"), join(served, "owl_wasm_bg.wasm"));
cpSync(join(built, "snippets"), join(served, "snippets"), { recursive: true });

const commit = execFileSync("git", ["-C", crate, "rev-parse", "--short", "HEAD"]).toString().trim();
const bytes = statSync(join(served, "owl_wasm_bg.wasm")).size;
writeFileSync(join(served, "build.json"), JSON.stringify({ builtFrom: commit, bytes }, null, 2) + "\n");
console.log(`\npublic/owl: engine built from ${commit}, ${(bytes / 1048576).toFixed(1)} MB`);
