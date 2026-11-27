import { Offstage } from "./Offstage";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { CUSTOMERS, ORDERS } from "../lib/shop";

// OQL has two forms, chain and block, and they mean the same thing. Every pair of queries in
// this section was run on a real owl-server and returned identical rows, and every error
// message quoted is the text the server sent back.

const k = (word: string) => `<span class="k">${word}</span>`;
const paid = `<span class="s">"paid"</span>`;

/* ---------- 1. The same query, piece by piece ---------- */

type Part = "root" | "where" | "sort" | "limit" | "select";

const PARTS: { id: Part; label: string; note: ReactNode }[] = [
  {
    id: "root",
    label: "orders",
    note: (
      <>
        Both start from a collection. A chain hangs steps off it with dots. A block opens a pair of braces and puts the
        steps inside.
      </>
    ),
  },
  {
    id: "where",
    label: "where",
    note: (
      <>
        <code>.where {"{ … }"}</code> becomes <code>where {"{ … }"}</code>. The dot goes, because the block already says
        which collection it is about.
      </>
    ),
  },
  { id: "sort", label: "sort", note: <>The same again. What is inside the braces does not change at all.</> },
  {
    id: "limit",
    label: "limit",
    note: (
      <>
        <code>.limit(3)</code> can be written <code>limit 3</code> in a block. The brackets are optional there.
      </>
    ),
  },
  {
    id: "select",
    label: "select",
    note: (
      <>
        <code>select</code> shapes the answer in both. In a block it is the last line.
      </>
    ),
  },
];

const CHAIN: [Part, string][] = [
  ["root", `orders\n`],
  ["where", `  .${k("where")} { .status == ${paid} }\n`],
  ["sort", `  .${k("sort")} { .total.desc }\n`],
  ["limit", `  .${k("limit")}(3)\n`],
  ["select", `  .${k("select")} { id, who: .customer.name, total }`],
];
const BLOCK: [Part, string][] = [
  ["root", `orders {\n`],
  ["where", `  ${k("where")} { .status == ${paid} }\n`],
  ["sort", `  ${k("sort")} { .total.desc }\n`],
  ["limit", `  ${k("limit")} 3\n`],
  ["select", `  ${k("select")} { id, who: .customer.name, total }\n`],
  ["root", `}`],
];

function Code({ title, pieces, active, pick }: { title: string; pieces: [Part, string][]; active: Part; pick: (p: Part) => void }) {
  return (
    <div className="code-card form-code">
      <span className="form-tag">{title}</span>
      <pre>
        <code>
          {pieces.map(([part, code], i) => (
            <span
              key={i}
              className={part === active ? "piece now" : "piece plain"}
              onMouseEnter={() => pick(part)}
              onClick={() => pick(part)}
              dangerouslySetInnerHTML={{ __html: code }}
            />
          ))}
        </code>
      </pre>
    </div>
  );
}

function SideBySide() {
  const [active, setActive] = useState<Part>("root");
  const [auto, setAuto] = useState(false);
  const box = useRef<HTMLDivElement>(null);

  // Cycle through the pieces while the comparison is on screen and nobody has taken over.
  useEffect(() => {
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const seen = new IntersectionObserver(([entry]) => setAuto(entry.isIntersecting), { threshold: 0.6 });
    seen.observe(box.current!);
    return () => seen.disconnect();
  }, []);
  useEffect(() => {
    if (!auto) return;
    const timer = setTimeout(() => {
      const at = PARTS.findIndex((p) => p.id === active);
      setActive(PARTS[(at + 1) % PARTS.length].id);
    }, 2800);
    return () => clearTimeout(timer);
  }, [auto, active]);

  const pick = (part: Part) => {
    setAuto(false);
    setActive(part);
  };

  return (
    <div className="pair" ref={box}>
      <Code title="Chain" pieces={CHAIN} active={active} pick={pick} />
      <div className="pair-middle">
        <span className="pair-label">One query</span>
        {PARTS.map((p) => (
          <button key={p.id} className={p.id === active ? "on" : ""} onClick={() => pick(p.id)} onMouseEnter={() => pick(p.id)}>
            {p.label}
          </button>
        ))}
      </div>
      <Code title="Block" pieces={BLOCK} active={active} pick={pick} />
      <p className="pair-note" key={active} aria-live="polite">
        {PARTS.find((p) => p.id === active)!.note}
      </p>
      <div className="tour-answer on pair-answer">
        <span>Owl answers, either way</span>
        <pre>{`{ id: "o1", who: "Ada", total: 125.0 }\n{ id: "o6", who: "Ada", total: 97.0 }\n{ id: "o2", who: "Bola", total: 85.0 }`}</pre>
      </div>
    </div>
  );
}

/* ---------- 2. What a block adds: names ---------- */

const REP = `<span class="rep">.orders.${k("where")} { .status == ${paid} }</span>`;
const NAME = `<span class="name">paid</span>`;
const SPENT = `<span class="name">spent</span>`;

const GROW = [
  {
    button: "One fact",
    times: 1,
    note: <>Each customer's paid orders, counted. So far the chain is no longer than the block.</>,
    chain: `customers\n  .${k("sort")} { .id.asc }\n  .${k("select")} {\n    name,\n    orders: ${REP}.${k("count")}()\n  }`,
    block: `customers {\n  ${NAME} = ${REP}\n\n  ${k("sort")} { .id.asc }\n  ${k("select")} { name, orders: ${NAME}.${k("count")}() }\n}`,
    answer: [`{ name: "Ada", orders: 2 }`, `{ name: "Bola", orders: 1 }`, `{ name: "Chen", orders: 1 }`],
  },
  {
    button: "Used twice",
    times: 2,
    note: <>Add what they spent. The chain writes the paid orders out a second time. The block uses the name again.</>,
    chain: `customers\n  .${k("sort")} { .id.asc }\n  .${k("select")} {\n    name,\n    orders: ${REP}.${k("count")}(),\n    spent: ${REP}.${k("sum")}(.total)\n  }`,
    block: `customers {\n  ${NAME} = ${REP}\n\n  ${k("sort")} { .id.asc }\n  ${k("select")} {\n    name,\n    orders: ${NAME}.${k("count")}(),\n    spent: ${NAME}.${k("sum")}(.total)\n  }\n}`,
    answer: [`{ name: "Ada", orders: 2, spent: 222.0 }`, `{ name: "Bola", orders: 1, spent: 85.0 }`, `{ name: "Chen", orders: 1, spent: 52.0 }`],
  },
  {
    button: "Used three times",
    times: 3,
    note: <>Keep only those who spent over 80. A third copy in the chain. In the block, a name built from a name.</>,
    chain: `customers\n  .${k("where")} {\n    ${REP}.${k("sum")}(.total) > 80\n  }\n  .${k("sort")} { .id.asc }\n  .${k("select")} {\n    name,\n    orders: ${REP}.${k("count")}(),\n    spent: ${REP}.${k("sum")}(.total)\n  }`,
    block: `customers {\n  ${NAME} = ${REP}\n  ${SPENT} = ${NAME}.${k("sum")}(.total)\n\n  ${k("where")} { ${SPENT} > 80 }\n  ${k("sort")} { .id.asc }\n  ${k("select")} {\n    name,\n    orders: ${NAME}.${k("count")}(),\n    spent: ${SPENT}\n  }\n}`,
    answer: [`{ name: "Ada", orders: 2, spent: 222.0 }`, `{ name: "Bola", orders: 1, spent: 85.0 }`],
  },
];

/** Each customer with the orders `paid` names, and what is worked out from them. */
function PaidDrawing({ step }: { step: number }) {
  return (
    <svg className="tv" viewBox="0 0 960 196" role="img" aria-label="Each customer's paid orders, their count and their total">
      <g className="tv-head">
        <text x={30} y={22}>customer</text>
        <text x={210} y={22}>paid</text>
        <text x={640} y={22} textAnchor="end">orders</text>
        <text x={800} y={22} textAnchor="end" style={{ opacity: step >= 1 ? 1 : 0 }}>spent</text>
      </g>
      {CUSTOMERS.map((c, row) => {
        const mine = ORDERS.filter((o) => o.customer === c.id && o.status === "paid");
        const spent = mine.reduce((sum, o) => sum + o.total, 0);
        const out = step >= 2 && spent <= 80;
        return (
          <g key={c.id} style={{ transform: `translate(0px, ${34 + row * 52}px)`, opacity: out ? 0.22 : 1 }}>
            <g className="tv-row">
              <rect x={20} width={150} height={38} rx={10} />
              <text x={36} y={24}>{c.name}</text>
            </g>
            <path className="tv-rule strong" d="M170 19 H200" />
            {mine.map((o, i) => (
              <g key={o.id} className="tv-row lit" style={{ transform: `translate(${200 + i * 120}px, 2px)` }}>
                <rect width={108} height={34} rx={10} />
                <text x={12} y={22}>{o.id}</text>
                <text x={96} y={22} textAnchor="end">{o.total}</text>
              </g>
            ))}
            <text className="tv-strong" x={640} y={24} textAnchor="end">{mine.length}</text>
            <text className="tv-strong" x={800} y={24} textAnchor="end" style={{ opacity: step >= 1 ? 1 : 0 }}>{spent}</text>
            <text className="tv-note" x={820} y={24} style={{ opacity: step >= 2 ? 1 : 0 }}>
              {spent > 80 ? "over 80" : "not over 80"}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

function Names() {
  const [step, setStep] = useState(0);
  const now = GROW[step];
  return (
    <div className="names">
      <div className="tour-rail">
        {GROW.map((g, i) => (
          <button key={g.button} aria-pressed={i === step} aria-selected={i === step} onClick={() => setStep(i)}>
            <b>0{i + 1}</b> {g.button}
          </button>
        ))}
      </div>
      {GROW.map((other, i) =>
        i === step ? null : (
          <Offstage key={other.button} title={other.button} code={[other.chain, other.block]} notes={[other.note]} answer={other.answer} />
        ),
      )}
      <p className="names-note" key={step}>
        {now.note}
      </p>
      <div className="names-pair">
        <div className="code-card form-code">
          <span className="form-tag">Chain</span>
          <span className="count-tag warn" key={`c${step}`}>
            paid orders written {now.times === 1 ? "once" : `${now.times} times`}
          </span>
          <pre>
            <code dangerouslySetInnerHTML={{ __html: now.chain }} />
          </pre>
        </div>
        <div className="code-card form-code">
          <span className="form-tag">Block</span>
          <span className="count-tag">paid orders written once</span>
          <pre>
            <code dangerouslySetInnerHTML={{ __html: now.block }} />
          </pre>
        </div>
      </div>
      <div className="tour-visual names-drawing">
        <PaidDrawing step={step} />
      </div>
      <div className="tour-answer on">
        <span>Owl answers, either way</span>
        <pre>{now.answer.join("\n")}</pre>
      </div>
    </div>
  );
}

/* ---------- 3. The rules of a block ---------- */

const RULES: { title: string; body: ReactNode; code: string; error?: string }[] = [
  {
    title: "It reads top to bottom",
    body: <>Names first, then the steps in the order you want them, then the answer.</>,
    code: `customers {\n  ${NAME} = ${REP}\n  ${k("where")} { ${NAME}.${k("count")}() > 0 }\n  ${k("select")} { name }\n}`,
  },
  {
    title: "It ends with the answer",
    body: (
      <>
        A block finishes with <code>select</code>, or with a closing step such as <code>count()</code>. Nothing comes
        after it inside the block.
      </>
    ),
    code: `customers {\n  ${k("select")} { name, country }\n  <span class="bad">${k("where")} { .country == <span class="s">"NG"</span> }</span>\n}`,
    error: "select must be the final entry in a scoped query block",
  },
  {
    title: "One name, one meaning",
    body: <>A name cannot be given twice, and it cannot reuse the name of the collection the block is about.</>,
    code: `customers {\n  ${NAME} = ${REP}\n  <span class="bad">${NAME} = .orders</span>\n  ${k("select")} { name }\n}`,
    error: "Scoped binding 'paid' is already defined",
  },
  {
    title: "Names stay inside",
    body: (
      <>
        A block gives rows, and a chain can carry on from them after it closes. Its names cannot: they ended with the
        block. Put the value in the block's <code>select</code> and read that.
      </>
    ),
    code: `customers {\n  ${NAME} = ${REP}\n  ${k("select")} { name }\n}<span class="bad">.${k("where")} { ${NAME}.${k("count")}() > 0 }</span>`,
    error: "'paid' is a name of the block, and the block has closed",
  },
  {
    title: "The collection's name means this row",
    body: (
      <>
        Inside a nested query, <code>.</code> is the nested row. The outer collection's name still reaches the row the
        block is on.
      </>
    ),
    code: `customers {\n  ${k("sort")} { .id.asc }\n  ${k("select")} {\n    name,\n    n: orders\n      .${k("where")} { .customer == <span class="name">customers.id</span> }\n      .${k("count")}()\n  }\n}`,
  },
];

function Rules() {
  return (
    <div className="rules">
      {RULES.map((rule, i) => (
        <article key={rule.title} className="rule">
          <span className="num">0{i + 1}</span>
          <h4>{rule.title}</h4>
          <p>{rule.body}</p>
          <pre>
            <code dangerouslySetInnerHTML={{ __html: rule.code }} />
          </pre>
          {rule.error && (
            <p className="rule-error">
              <b>Owl says</b> {rule.error}
            </p>
          )}
        </article>
      ))}
    </div>
  );
}

/* ---------- The section ---------- */

export function Forms() {
  return (
    <section className="band" id="forms">
      <div className="wrap">
        <p className="eyebrow">Two forms</p>
        <h2>
          Chain it or block it.
          <br />
          Owl reads one query.
        </h2>
        <p className="sub">
          Every query can be written as a chain of steps or as a block. They are the same language and give the same
          answer. Pick the one that reads better.
        </p>

        <h3 className="forms-h">The same query, piece by piece</h3>
        <SideBySide />

        <h3 className="forms-h">What a block adds: names</h3>
        <p className="forms-p">
          A block can give things names, and a chain cannot. With <code>=</code> a name stands for a whole value: write
          it once and use it wherever it is needed, as below. With <code>in</code> a name stands for each row of a
          collection in turn, which the Pair chapter of the tour walks through.
        </p>
        <Names />

        <h3 className="forms-h">The rules of a block</h3>
        <Rules />

        <h3 className="forms-h">Which to reach for</h3>
        <div className="reach">
          <article>
            <span className="form-tag dark">Chain</span>
            <p>
              For a short run of steps. A chain also carries on after a block closes, to sort or limit by a field its{" "}
              <code>select</code> just worked out.
            </p>
          </article>
          <article>
            <span className="form-tag dark">Block</span>
            <p>
              When the query uses the same fact more than once, when naming the parts makes a long query easier to
              read, or when the answer puts several rows side by side, each named with <code>in</code>.
            </p>
          </article>
        </div>
      </div>
    </section>
  );
}
