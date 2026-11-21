// The engine, in a worker: a browser gives files that can be read and
// written without waiting only to a worker. The page sends what to run and
// is sent back what came of it.
import init, { Owl } from "./owl_wasm.js";

const FOLDER = "owl-playground";
// The pool begins with this many files and is given more while it has
// fewer than `SPARE` holding nothing. The engine takes one for each file
// it makes and cannot wait for one.
const FIRST = 32, SPARE = 12, MORE = 16;
// Nothing checkpoints of its own accord here, so the log is folded into
// the checkpoint after this many runs.
const CHECKPOINT_EVERY = 20;

let owl = null, folder = null, handles = [], kept = false, why = "", runs = 0;

async function open(from, to) {
  const opened = [];
  for (let number = from; number < to; number++) {
    const file = await folder.getFileHandle(`file-${number}`, { create: true });
    opened.push(await file.createSyncAccessHandle());
  }
  return opened;
}

function close() {
  if (owl) owl.free();
  owl = null;
  for (const handle of handles) {
    try { handle.close(); } catch {}
  }
  handles = [];
}

async function start() {
  close();
  kept = false;
  why = "";
  try {
    const root = await navigator.storage.getDirectory();
    folder = await root.getDirectoryHandle(FOLDER, { create: true });
    let there = 0;
    for await (const _name of folder.keys()) there++;
    handles = await open(0, Math.max(there, FIRST));
    owl = Owl.openKept(handles);
    kept = true;
  } catch (error) {
    // Another tab has the files, or this browser has no such file system:
    // the database is held in memory, and the page says so.
    close();
    why = /Access Handle/.test(String(error?.message)) ? "another tab" : "no files";
    owl = new Owl();
  }
}

async function topUp() {
  if (!kept || owl.freeSlots() >= SPARE) return;
  const more = await open(handles.length, handles.length + MORE);
  handles.push(...more);
  owl.addSlots(more);
}

function about() {
  const files = kept ? JSON.parse(owl.files()) : [];
  return { kept, why, files: files.length, bytes: files.reduce((total, file) => total + file.size, 0) };
}

const ready = init().then(start);

self.onmessage = async ({ data: { id, kind, source } }) => {
  let answer;
  try {
    await ready;
    if (kind === "run") {
      const started = performance.now();
      const result = JSON.parse(owl.run(source));
      result.ms = performance.now() - started;
      if (kept && ++runs % CHECKPOINT_EVERY === 0) owl.checkpoint();
      await topUp();
      answer = { result, about: about() };
    } else if (kind === "forget") {
      // Start over: close the files and remove them, then begin again.
      close();
      if (folder) {
        const root = await navigator.storage.getDirectory();
        await root.removeEntry(FOLDER, { recursive: true }).catch(() => {});
      }
      await start();
      answer = { about: about() };
    } else {
      answer = { about: about() };
    }
  } catch (error) {
    answer = {
      result: { rows: [], columns: [], ran: 0, errors: [String(error?.message ?? error)], ms: 0 },
      about: owl ? about() : { kept: false, why: "no engine", files: 0, bytes: 0 },
    };
  }
  self.postMessage({ id, ...answer });
};
