# Owl in the browser

The engine compiled to WebAssembly, with a page to try it in. It is the same
code the server runs. The database is held in memory, so it is gone when the
page is closed.

## Try it

```sh
wasm-pack build apps/owl-wasm --target web --release --out-dir pkg
node apps/owl-wasm/demo/serve.mjs
```

Then open the address it prints. The page loads a small library of books and
has samples to run; anything else typed in is run the same way.

## From JavaScript

```js
import init, { Owl } from "./pkg/owl_wasm.js";

await init();
const owl = new Owl();
const answer = JSON.parse(owl.run(`
  schema books { id: String, title: String, year: Integer }
  books.insert [ { id: "b1", title: "Solaris", year: 1961 } ]
  books.where { .year < 1970 }.select { title }
`));
// { rows: [{ title: "Solaris" }], columns: ["title"], ran: 3, errors: [] }
```

`run` takes schemas, inserts, functions and queries, one after another. It
gives the rows of the last statement when that is a query, the columns in the
order the query selected them, how many statements ran, and what went wrong.
It does not throw for a mistake in the OQL.

## What is different in a browser

- **Nothing is kept.** The files a database is made of are held in memory.
  Keeping them in the browser's own file system is the next step.
- **No threads.** Nothing checkpoints of its own accord, and one query runs at
  a time.
- **Thirty-two bits.** A database is at most 4 GB, and so is memory.
- **The clock and randomness come from the browser.**

The build is about 6.4 MB, 1.7 MB compressed, and has not been made smaller.
