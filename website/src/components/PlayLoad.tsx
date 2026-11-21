import { useState } from "react";
import type { Engine, Collection } from "../lib/owl";
import { guess, inserts, named, readRows, schemaText } from "../lib/readRows";
import type { Guess } from "../lib/readRows";

/** More than this is more than a page should be asked to paste and parse. */
const MOST_TEXT = 3 * 1024 * 1024;
const MOST_ROWS = 20_000;
/** A collection with more rows than this is not looked through for references. */
const MOST_IDS = 20_000;

const EXAMPLE_NAME = "reviews";
const EXAMPLE = `[
  { "id": "r1", "customer": "ada", "stars": 5, "text": "Quick and tidy.", "at": "2026-03-20T09:30:00Z", "tags": ["fast", "neat"] },
  { "id": "r2", "customer": "chen", "stars": 3, "text": "Fine.", "at": "2026-03-21T11:00:00Z", "tags": [] },
  { "id": "r3", "customer": "ada", "stars": 4, "text": "Would order again.", "at": "2026-03-22T08:15:00Z", "tags": ["again"] },
  { "id": "r4", "customer": "bola", "stars": 2, "text": "Arrived late.", "at": "2026-03-25T17:40:00Z", "tags": ["late"] }
]`;

type ReadIn = { rows: Array<Record<string, unknown>>; fields: Record<string, Guess>; notes: string[] };
type Outcome = { said: "Loaded" | "Refused"; lines: string[] };

/** Rows a visitor brings: read, given a schema they can change, checked by the engine, and loaded whole or not at all. */
export function PlayLoad({
  engine,
  collections,
  onLoaded,
  onWrite,
}: {
  engine: Engine | null;
  collections: Collection[];
  onLoaded: (collection: string) => void;
  onWrite: (code: string) => void;
}) {
  const [name, setName] = useState("");
  const [pasted, setPasted] = useState("");
  const [read, setRead] = useState<ReadIn | null>(null);
  const [schema, setSchema] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [replace, setReplace] = useState(false);
  const [busy, setBusy] = useState(false);

  const collection = named(name || "things").toLowerCase();
  // The collection the schema names is the one loaded into, whatever the box above says.
  const target = /schema\s+([A-Za-z][A-Za-z0-9_]*)/.exec(schema)?.[1] ?? collection;
  const taken = collections.some((c) => c.name === target);

  const readIt = async (text = pasted, into = collection) => {
    setOutcome(null);
    setRead(null);
    setProblem(null);
    if (!engine) return;
    try {
      if (text.length > MOST_TEXT) throw new Error("That is more than 3 MB. This page is for trying Owl out; load less.");
      const { rows, notes } = readRows(text);
      if (rows.length > MOST_ROWS) throw new Error(`That is ${rows.length} rows. This page takes ${MOST_ROWS} at a time.`);
      setBusy(true);
      // The ids already in the database, to see which fields are references to them.
      const ids: Record<string, Set<string>> = {};
      for (const c of collections) {
        if (c.name === into || (c.rows ?? 0) > MOST_IDS) continue;
        const { answer } = await engine.run(`${c.name}.select { id }`);
        if (!answer.errors.length) ids[c.name] = new Set(answer.rows.map((row) => String(row.id)));
      }
      const fields = guess(rows, ids);
      for (const [field, guessed] of Object.entries(fields)) {
        if (guessed.ref) notes.push(`Every ${field} is the id of a row in ${guessed.ref}, so it is a reference to it.`);
        if (guessed.optional) notes.push(`${field} is missing from some rows, so it is optional.`);
      }
      setRead({ rows, fields, notes });
      setSchema(schemaText(into, fields));
    } catch (error) {
      setProblem((error as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const loadIt = async () => {
    if (!engine || !read) return;
    setBusy(true);
    setOutcome(null);
    const refuse = (lines: string[]) => setOutcome({ said: "Refused", lines });
    try {
      if (taken && !replace) {
        return refuse([`There is already a collection called ${target}. Give yours another name, or tick the box to replace it.`]);
      }
      if (taken) {
        const dropped = await engine.run(`drop ${target}`);
        if (dropped.answer.errors.length) return refuse(dropped.answer.errors);
      }
      const declared = await engine.run(schema);
      if (declared.answer.errors.length) return refuse(declared.answer.errors);
      // Every row or none: one transaction, and the collection goes again if it does not hold.
      const loaded = await engine.run(`transaction {\n${inserts(target, read.rows, read.fields).join("\n")}\n}`);
      if (loaded.answer.errors.length) {
        await engine.run(`drop ${target}`);
        return refuse([
          ...loaded.answer.errors.filter((line) => !/^Transaction rolled back/.test(line)).map((line) => line.replace(/^Execution failed: /, "")),
          "Nothing was loaded. Change the schema or the rows and try again.",
        ]);
      }
      setOutcome({ said: "Loaded", lines: [`${read.rows.length} ${read.rows.length === 1 ? "row is" : "rows are"} in ${target}, and kept with the rest.`] });
      onLoaded(target);
    } finally {
      setBusy(false);
    }
  };

  const example = () => {
    setName(EXAMPLE_NAME);
    setPasted(EXAMPLE);
    readIt(EXAMPLE, EXAMPLE_NAME);
  };

  return (
    <section className="band load-band" id="yours">
      <div className="wrap">
        <p className="eyebrow">Your data</p>
        <h2>
          Bring
          <br />
          your own.
        </h2>
        <p className="sub">
          Paste rows as JSON or CSV. Owl works out a schema you can change, checks every row against it, and loads all
          of them or none. It stays in your browser.
        </p>

        <div className="load-stage">
          <div className="load-left">
            <span className="form-tag dark">1. Paste</span>
            <label className="load-name">
              <span>Call the collection</span>
              <input
                value={name}
                onChange={(event) => setName(event.target.value)}
                placeholder="things"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
              />
            </label>
            <textarea
              className="load-paste"
              value={pasted}
              onChange={(event) => setPasted(event.target.value)}
              placeholder={`[\n  { "id": "a1", "name": "…", "price": 12.5 },\n  …\n]\n\nor\n\nid,name,price\na1,…,12.5`}
              spellCheck={false}
              aria-label="Rows to load, as JSON or CSV"
              rows={11}
            />
            <div className="load-buttons">
              <button className="btn solid" onClick={() => readIt()} disabled={busy || !engine}>
                Read it
              </button>
              <button className="btn" onClick={example} disabled={busy || !engine}>
                Show me one
              </button>
            </div>
            {problem && (
              <div className="verdict refused load-verdict play-shake" key={problem}>
                <span className="stamp-mark">Unread</span>
                <p>{problem}</p>
              </div>
            )}
            <p className="load-or">
              Or write a schema by hand:{" "}
              <button
                className="play-forget"
                onClick={() =>
                  onWrite(
                    `schema notes {\n  id: String,\n  text: String,\n  pinned: Boolean,\n  written: DateTime\n}\n\nnotes.insert { id: "n1", text: "first", pinned: true, written: "2026-03-01T09:00:00Z" }\n\nnotes.limit(10)`,
                  )
                }
              >
                start one in the editor
              </button>
              .
            </p>
          </div>

          <div className="load-right">
            <span className="form-tag dark">2. Check and load</span>
            {!read ? (
              <div className="verdict waiting load-verdict">
                <p>What Owl makes of your rows shows here: a schema to look over before anything is loaded.</p>
              </div>
            ) : (
              <>
                <p className="load-count">
                  <b>{read.rows.length}</b> {read.rows.length === 1 ? "row" : "rows"} read, <b>{Object.keys(read.fields).length}</b> fields.
                </p>
                {read.notes.length > 0 && (
                  <ul className="load-notes">
                    {read.notes.map((note) => (
                      <li key={note}>{note}</li>
                    ))}
                  </ul>
                )}
                <div className="code-card form-code load-schema">
                  <span className="form-tag">Schema</span>
                  <textarea
                    value={schema}
                    onChange={(event) => {
                      setSchema(event.target.value);
                      setOutcome(null);
                    }}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    aria-label="The schema your rows will be checked against"
                    rows={schema.split("\n").length}
                  />
                </div>
                {taken && (
                  <label className="load-replace">
                    <input type="checkbox" checked={replace} onChange={(event) => setReplace(event.target.checked)} />
                    <span>
                      There is already a collection called <b>{target}</b>. Replace it and its rows.
                    </span>
                  </label>
                )}
                <div className="load-buttons">
                  <button className="btn solid" onClick={loadIt} disabled={busy}>
                    Load {read.rows.length} {read.rows.length === 1 ? "row" : "rows"}
                  </button>
                </div>
                {outcome && (
                  <div className={`verdict load-verdict ${outcome.said === "Loaded" ? "done" : "refused play-shake"}`} key={outcome.lines.join()}>
                    <span className="stamp-mark">{outcome.said}</span>
                    {outcome.lines.map((line) => (
                      <p key={line}>{line}</p>
                    ))}
                    {outcome.said === "Loaded" && (
                      <p>
                        <a href="#ask">Ask it something.</a>
                      </p>
                    )}
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </section>
  );
}
