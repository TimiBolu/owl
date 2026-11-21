# Owl in the browser

The engine compiled to WebAssembly, with a page to try it in. It is the same
code the server runs. The database is kept in the browser's own file system,
so it is still there when the page is loaded again.

## Try it

```sh
wasm-pack build apps/owl-wasm --target web --release --out-dir pkg
node apps/owl-wasm/demo/serve.mjs
```

Then open the address it prints. A new database is given a small library of
books, and the page has samples to run; anything else typed in is run the
same way. Write something, reload, and it is still there.

## From JavaScript

A database held in memory, in a page or a worker:

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

A database that is kept, in a worker:

```js
const root = await navigator.storage.getDirectory();
const folder = await root.getDirectoryHandle("owl-database", { create: true });
const handles = [];
for (let number = 0; number < 32; number++) {
  const file = await folder.getFileHandle(`file-${number}`, { create: true });
  handles.push(await file.createSyncAccessHandle());
}
const owl = Owl.openKept(handles);
```

`demo/worker.js` is the whole of it: opening the files, giving the pool more
as it is used, a checkpoint now and then, and falling back to memory when
the files cannot be had.

## How a database is kept

The engine opens its files without waiting, and a browser gives a file that
can be read and written without waiting only after being waited on to open
it. So the files are opened first, a pool of them, and the engine's files
are kept in those: each file of the pool holds one of the engine's, under a
header that says which. Making a file takes a free one, removing a file
gives it back, and renaming a file rewrites a header, none of which waits.

- **The pool is finite.** The engine takes a file for each one it makes, and
  making one fails when none is left. A page gives the pool more while it
  still has some to spare (`freeSlots`, `addSlots`).
- **A name is written twice.** A header has two halves and a change goes to
  the half not in force, so one cut short leaves the name the file had.
- **One tab at a time.** A browser lets one holder have such a file. A second
  tab gets none and holds its database in memory.

## What is different in a browser

- **No threads.** Nothing checkpoints of its own accord, so a page that keeps
  its database asks for a checkpoint now and then (`checkpoint`). One query
  runs at a time.
- **Thirty-two bits.** A database is at most 4 GB, and so is memory.
- **The clock and randomness come from the browser.**

The build is about 6.4 MB, 1.7 MB compressed, and has not been made smaller.
