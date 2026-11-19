import { useEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";

// Every statement in this section was run on a real owl-server, and every answer and error
// message shown is what the server sent back. The schema shown is a trimmed copy of the one
// that was loaded.

const k = (word: string) => `<span class="k">${word}</span>`;
const s = (text: string) => `<span class="s">"${text}"</span>`;

/* ---------- 1. Four ways to change things ---------- */

type Cell = string | { was: string; now: string; kept?: boolean };
type Row = { cells: Cell[]; change?: "new" | "gone" };

type Verb = {
  name: string;
  title: string;
  code: string;
  about: ReactNode;
  table: string;
  columns: string[];
  rows: Row[];
  answer: string;
};

const VERBS: Verb[] = [
  {
    name: "Insert",
    title: "Add a row by writing its shape.",
    code: `customers.${k("insert")} {\n  id: ${s("chen")},\n  name: ${s("Chen")},\n  email: ${s("chen@shop.test")},\n  address: { city: ${s("Singapore")} }\n}`,
    about: (
      <>
        Give <code>insert</code> a list in square brackets and it adds them all, or none: one bad row in the list and
        nothing is added.
      </>
    ),
    table: "customers",
    columns: ["id", "name", "email"],
    rows: [
      { cells: ["ada", "Ada", "ada@shop.test"] },
      { cells: ["bola", "Bola", "bola@shop.test"] },
      { cells: ["chen", "Chen", "chen@shop.test"], change: "new" },
    ],
    answer: `{ id: "chen", name: "Chen", email: "chen@shop.test",\n  address: { city: "Singapore" } }`,
  },
  {
    name: "Update",
    title: "Find the rows, then say what changes.",
    code: `products\n  .${k("where")} { .id == ${s("knife")} }\n  .${k("update")} { stock -= 1 }`,
    about: (
      <>
        <code>stock -= 1</code> is short for <code>stock: .stock - 1</code>. A path such as{" "}
        <code>address.city: "Ibadan"</code> changes one part of a nested object and leaves the rest.
      </>
    ),
    table: "products",
    columns: ["id", "name", "price", "stock"],
    rows: [{ cells: ["kettle", "Kettle", "40.0", "2"] }, { cells: ["knife", "Chef knife", "85.0", { was: "5", now: "4" }] }],
    answer: `{ id: "knife", name: "Chef knife", price: 85.0, stock: 4 }`,
  },
  {
    name: "Upsert",
    title: "Add it, or change it if it is already there.",
    code: `customers.${k("upsert")} ${k("on")} id {\n  id: ${s("chen")},\n  name: ${s("Chen W.")}\n}`,
    about: (
      <>
        <code>on id</code> names the field that decides. No row with that id: Owl inserts. One row: Owl updates it and
        keeps every field you left out.
      </>
    ),
    table: "customers",
    columns: ["id", "name", "email"],
    rows: [
      { cells: ["ada", "Ada", "ada@shop.test"] },
      { cells: ["bola", "Bola", "bola@shop.test"] },
      { cells: ["chen", { was: "Chen", now: "Chen W." }, { was: "chen@shop.test", now: "chen@shop.test", kept: true }] },
    ],
    answer: `{ id: "chen", name: "Chen W.", email: "chen@shop.test",\n  address: { city: "Singapore" } }`,
  },
  {
    name: "Delete",
    title: "Find the rows, then remove them.",
    code: `orders\n  .${k("where")} { .status == ${s("cancelled")} }\n  .${k("delete")}()`,
    about: <>Every write answers with the rows it touched, so you can see exactly what was removed.</>,
    table: "orders",
    columns: ["id", "customer", "status"],
    rows: [{ cells: ["o1", "ada", "paid"] }, { cells: ["o2", "bola", "cancelled"], change: "gone" }],
    answer: `{ id: "o2", customer: "bola", product: "knife",\n  qty: 1, status: "cancelled" }`,
  },
];

function Verbs() {
  const [at, setAt] = useState(0);
  const [ran, setRan] = useState(false);
  const verb = VERBS[at];

  // Show the table as it was, then run the statement a moment later.
  useEffect(() => {
    if (ran) return;
    const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const timer = setTimeout(() => setRan(true), calm ? 0 : 1300);
    return () => clearTimeout(timer);
  }, [ran, at]);

  const open = (n: number) => {
    setAt(n);
    setRan(false);
  };

  return (
    <div className="verbs">
      <div className="tour-rail" role="tablist" aria-label="Ways to change data">
        {VERBS.map((v, i) => (
          <button key={v.name} role="tab" aria-selected={i === at} onClick={() => open(i)}>
            <b>0{i + 1}</b> {v.name}
          </button>
        ))}
      </div>
      <h3 className="tour-title">{verb.title}</h3>
      <div className="kinds-stage">
        <div className="code-card form-code verb-code">
          <span className="form-tag">You write</span>
          <pre>
            <code dangerouslySetInnerHTML={{ __html: verb.code }} />
          </pre>
          <button className="run" onClick={() => setRan(false)}>
            Run it again
          </button>
        </div>
        <div>
          <div className="tour-visual wtable" key={verb.name} style={{ "--cols": verb.columns.length } as CSSProperties}>
            <p className="wtable-name">{verb.table}</p>
            <div className="wrow whead">
              {verb.columns.map((c) => (
                <span key={c}>{c}</span>
              ))}
            </div>
            {verb.rows.map((row, i) => {
              const state = !ran ? (row.change === "new" ? "hidden" : "") : (row.change ?? "");
              return (
                <div key={i} className={`wrow ${state}`}>
                  {row.cells.map((cell, n) =>
                    typeof cell === "string" ? (
                      <span key={n}>{cell}</span>
                    ) : (
                      <span key={n}>
                        <em className={ran ? (cell.kept ? "stayed" : "changed") : ""}>{ran ? cell.now : cell.was}</em>
                      </span>
                    ),
                  )}
                </div>
              );
            })}
          </div>
          <div className={ran ? "tour-answer on" : "tour-answer"}>
            <span>Owl answers with the row it touched</span>
            <pre>{verb.answer}</pre>
          </div>
        </div>
      </div>
      <p className="kinds-about" key={at}>
        {verb.about}
      </p>
    </div>
  );
}

/* ---------- 2. Rules that live in the schema ---------- */

type Line = "unique" | "check" | "ref" | "type" | "cascade";

const SCHEMA: [Line | null, string][] = [
  [null, `${k("schema")} customers {\n  id: String,\n  name: String,\n`],
  ["unique", `  email: String ${k("unique")}\n`],
  [null, `}\n\n${k("schema")} products {\n  id: String,\n  price: Float,\n  stock: Integer,\n`],
  ["check", `  ${k("check")}(.price >= 0 && .stock >= 0)\n`],
  [null, `}\n\n${k("schema")} orders {\n  id: String,\n`],
  ["ref", `  customer: ${k("Ref")}(customers),\n`],
  ["type", `  qty: Integer\n`],
  [null, `}\n\n${k("schema")} payments {\n  id: String,\n`],
  ["cascade", `  order: ${k("Ref")}(orders) ${k("on delete cascade")}\n`],
  [null, `}`],
];

const TRIES: { label: string; line: Line; code: string; refused: boolean; says: string }[] = [
  {
    label: "Use Ada's email for someone else",
    line: "unique",
    code: `customers.${k("insert")} {\n  id: ${s("eve")}, name: ${s("Eve")},\n  email: ${s("ada@shop.test")},\n  address: { city: ${s("Accra")} }\n}`,
    refused: true,
    says: `'customers' document 'eve' has email "ada@shop.test", which must be unique, and another document already has it`,
  },
  {
    label: "Set a price below zero",
    line: "check",
    code: `products\n  .${k("where")} { .id == ${s("kettle")} }\n  .${k("update")} { price: -5.0 }`,
    refused: true,
    says: "'products' document 'kettle' fails check(.price >= 0 && .stock >= 0)",
  },
  {
    label: "Order for a customer who is not there",
    line: "ref",
    code: `orders.${k("insert")} {\n  id: ${s("o9")}, customer: ${s("nobody")},\n  product: ${s("kettle")}, qty: 1,\n  status: ${s("open")}\n}`,
    refused: true,
    says: "Referenced document not found: nobody in customers",
  },
  {
    label: "Put a word where a number goes",
    line: "type",
    code: `orders.${k("insert")} {\n  id: ${s("o8")}, customer: ${s("ada")},\n  product: ${s("kettle")}, qty: ${s("two")},\n  status: ${s("open")}\n}`,
    refused: true,
    says: "Expected Integer, got String",
  },
  {
    label: "Delete a customer who has orders",
    line: "ref",
    code: `customers\n  .${k("where")} { .id == ${s("ada")} }\n  .${k("delete")}()`,
    refused: true,
    says: "cannot delete 'customers' document 'ada': 'orders' document 'o1' refers to it through 'customer'. Delete or repoint that document first, or declare the reference 'on delete cascade' or 'on delete nullify'",
  },
  {
    label: "Delete an order that has payments",
    line: "cascade",
    code: `orders\n  .${k("where")} { .id == ${s("o1")} }\n  .${k("delete")}()`,
    refused: false,
    says: "The order is gone, and so are its two payments. The schema said they go with it.",
  },
];

function Rules() {
  const [at, setAt] = useState(0);
  const attempt = TRIES[at];
  return (
    <div className="guard">
      <div className="code-card form-code guard-schema">
        <span className="form-tag">The schema</span>
        <pre>
          <code>
            {SCHEMA.map(([line, code], i) => (
              <span key={i} className={line === attempt.line ? "piece now" : "piece plain"} dangerouslySetInnerHTML={{ __html: code }} />
            ))}
          </code>
        </pre>
      </div>
      <div className="guard-right">
        <p className="guard-label">Try to break it</p>
        <div className="guard-tries">
          {TRIES.map((t, i) => (
            <button key={t.label} className={i === at ? "on" : ""} onClick={() => setAt(i)}>
              {t.label}
            </button>
          ))}
        </div>
        <div className="code-card form-code guard-try" key={at}>
          <span className="form-tag">You write</span>
          <pre>
            <code dangerouslySetInnerHTML={{ __html: attempt.code }} />
          </pre>
        </div>
        <div className={attempt.refused ? "verdict refused" : "verdict done"} key={`v${at}`}>
          <span className="stamp-mark">{attempt.refused ? "Refused" : "Done"}</span>
          <p>
            {attempt.refused && <b>Owl says </b>}
            {attempt.says}
          </p>
        </div>
      </div>
    </div>
  );
}

/* ---------- 3. All or nothing ---------- */

function OneStatement() {
  const [stock, setStock] = useState(2);
  const [log, setLog] = useState<{ ok: boolean; text: string }[]>([]);
  const buy = () => {
    if (stock > 0) {
      setLog((l) => [...l, { ok: true, text: `1 row came back. Stock is now ${stock - 1}.` }].slice(-3));
      setStock(stock - 1);
    } else {
      setLog((l) => [...l, { ok: false, text: "No rows came back. Nothing matched, so nothing changed." }].slice(-3));
    }
  };
  return (
    <div className="atom">
      <div className="code-card form-code">
        <span className="form-tag">One statement</span>
        <pre>
          <code
            dangerouslySetInnerHTML={{
              __html: `products\n  .${k("where")} { .id == ${s("kettle")} && .stock > 0 }\n  .${k("update")} { stock -= 1 }`,
            }}
          />
        </pre>
      </div>
      <div className="atom-play">
        <div className="shelf" aria-label={`${stock} kettles in stock`}>
          {[0, 1].map((i) => (
            <span key={i} className={i < stock ? "kettle" : "kettle sold"} />
          ))}
          <b key={stock}>{stock}</b>
          <small>kettles in stock</small>
        </div>
        <div className="atom-buttons">
          <button className="btn solid" onClick={buy}>
            Buy a kettle
          </button>
          <button
            className="btn"
            onClick={() => {
              setStock(2);
              setLog([]);
            }}
          >
            Restock
          </button>
        </div>
        <ul className="atom-log" aria-live="polite">
          {log.map((entry, i) => (
            <li key={`${i}${entry.text}`} className={entry.ok ? "" : "no"}>
              {entry.text}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

type Run = "idle" | "plain" | "transaction";

function Together() {
  const [run, setRun] = useState<Run>("idle");
  const [step, setStep] = useState(0);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);

  const start = (mode: Run) => {
    timers.current.forEach(clearTimeout);
    setRun(mode);
    setStep(0);
    const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;
    timers.current = [1, 2, 3].map((n) => setTimeout(() => setStep(n), calm ? 0 : n * 1100));
  };
  useEffect(() => () => timers.current.forEach(clearTimeout), []);

  const orderShown = run !== "idle" && step >= 1 && !(run === "transaction" && step >= 3);
  const failed = run !== "idle" && step >= 2;

  return (
    <div className="together">
      <div className="code-card form-code">
        <span className="form-tag">Two statements that belong together</span>
        <pre>
          <code>
            <span className={run === "transaction" ? "piece now" : "piece later"} dangerouslySetInnerHTML={{ __html: `${k("transaction")} {\n` }} />
            <span
              className={step === 1 ? "piece now" : "piece plain"}
              dangerouslySetInnerHTML={{
                __html: `  orders.${k("insert")} {\n    id: ${s("o4")}, customer: ${s("ada")},\n    product: ${s("kettle")}, qty: 2, status: ${s("paid")}\n  }\n`,
              }}
            />
            <span
              className={step >= 2 ? "piece now bad-line" : "piece plain"}
              dangerouslySetInnerHTML={{ __html: `  products\n    .${k("where")} { .id == ${s("kettle")} }\n    .${k("update")} { stock -= 2 }\n` }}
            />
            <span className={run === "transaction" ? "piece now" : "piece later"}>{"}"}</span>
          </code>
        </pre>
        <div className="together-buttons">
          <button className="run" onClick={() => start("plain")}>
            Run them one by one
          </button>
          <button className="run strong" onClick={() => start("transaction")}>
            Run them in a transaction
          </button>
        </div>
      </div>

      <div className="together-right">
        <div className="tour-visual ledger">
          <div>
            <p className="wtable-name">orders</p>
            <span className="chip">o1</span>
            <span className="chip">o2</span>
            <span className="chip">o3</span>
            <span className={orderShown ? (failed && run === "plain" ? "chip new orphan" : "chip new") : "chip new off"}>o4</span>
          </div>
          <div>
            <p className="wtable-name">kettle stock</p>
            <span className={failed && step === 2 ? "stock shake" : "stock"} key={`${run}${step === 2}`}>1</span>
            <small>the order wants 2</small>
          </div>
        </div>
        <div className={`verdict ${run === "idle" || step < 3 ? "waiting" : run === "plain" ? "refused" : "done"}`} key={`${run}${step >= 3}`}>
          {run === "idle" && <p>Only one kettle is left, and Ada wants two. Run it both ways.</p>}
          {run !== "idle" && step < 2 && <p>The order goes in.</p>}
          {run !== "idle" && step === 2 && (
            <p>
              <b>Owl says </b>'products' document 'kettle' fails check(.price &gt;= 0 &amp;&amp; .stock &gt;= 0)
            </p>
          )}
          {run === "plain" && step >= 3 && (
            <>
              <span className="stamp-mark">Half done</span>
              <p>The stock change was refused, but the order had already gone in. There is now an order with no kettles behind it.</p>
            </>
          )}
          {run === "transaction" && step >= 3 && (
            <>
              <span className="stamp-mark">Nothing happened</span>
              <p>
                <b>Owl says </b>Transaction rolled back due to 1 error(s). The order was taken back out, so the data is exactly as it was.
              </p>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/* ---------- The section ---------- */

export function Writing() {
  return (
    <section className="band" id="writing">
      <div className="wrap">
        <p className="eyebrow">Writing</p>
        <h2>
          Change data in
          <br />
          the same language.
        </h2>
        <p className="sub">
          Writes start from a collection, like reads do, and use the same <code>where</code>. The schema holds the
          rules, and Owl refuses any write that would break one.
        </p>

        <Verbs />

        <h3 className="forms-h">Rules that live in the schema</h3>
        <p className="forms-p">
          You say once what must always be true. Owl checks it on every write, whichever program is asking.
        </p>
        <Rules />

        <h3 className="forms-h">One statement is all or nothing</h3>
        <p className="forms-p">
          The test and the change happen together, so two buyers cannot both take the last kettle. Most updates need
          nothing more than this.
        </p>
        <OneStatement />

        <h3 className="forms-h">A transaction makes several statements one</h3>
        <p className="forms-p">
          When changes only make sense together, put them in a <code>transaction</code> block. If any one fails, none
          of them happened.
        </p>
        <Together />
      </div>
    </section>
  );
}
