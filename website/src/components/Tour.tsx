import { Offstage } from "./Offstage";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { BucketsVisual, LinksVisual, NearVisual, PairsVisual, PipelineVisual, PlaceVisual, SchemaVisual, TreeVisual } from "./TourVisuals";
import { SOUGHT } from "../lib/shop";

/**
 * A piece of a chapter's code. `from` is the step that introduces it: before that step it is
 * faded, during it (through `to`) it is highlighted, and afterwards it reads as plain code.
 * Pieces without `from` are always plain.
 */
type Piece = { code: string; from?: number; to?: number };

type Chapter = {
  name: string;
  title: string;
  code: Piece[];
  steps: ReactNode[];
  answer?: string[];
  visual: (stage: number) => ReactNode;
};

const k = (word: string) => `<span class="k">${word}</span>`;
const s = (text: string) => `<span class="s">"${text}"</span>`;

// Every query below, and every answer, was run against a real owl-server holding the rows in
// src/lib/shop.ts. The schema chapter shows a trimmed version of the schema that was loaded.
const CHAPTERS: Chapter[] = [
  {
    name: "Describe",
    title: "Say what your data looks like.",
    code: [
      { from: 0, code: `${k("schema")} customers {\n  id: String,\n  name: String,\n  country: String\n}\n\n` },
      { from: 1, code: `${k("schema")} orders {\n  id: String,\n  status: String,\n  total: Float,\n` },
      { from: 2, to: 3, code: `  customer: ${k("Ref")}(customers),\n` },
      { from: 4, code: `  items: ${k("List")}({\n    product: ${k("Ref")}(products),\n    qty: Integer\n  })\n` },
      { from: 1, code: `}` },
    ],
    steps: [
      <>A schema names a collection and the type of each field.</>,
      <>A second collection, with plain fields so far.</>,
      <>
        <code>Ref</code> makes a field point at another collection. From an order, <code>.customer</code> is now a step you can take.
      </>,
      <>
        You get the way back for free. Every customer has <code>.orders</code>, and Owl keeps an index for it.
      </>,
      <>A list of objects lives inside its order. No second collection, no join table.</>,
    ],
    visual: (stage) => <SchemaVisual stage={stage} />,
  },
  {
    name: "Ask",
    title: "A query is a chain of small steps.",
    code: [
      { from: 0, code: `orders\n` },
      { from: 1, code: `  .${k("where")} { .status == ${s("paid")} }\n` },
      { from: 2, code: `  .${k("sort")} { .total.desc }\n` },
      { from: 3, code: `  .${k("limit")}(3)\n` },
      { from: 4, code: `  .${k("select")} { id, total }` },
    ],
    steps: [
      <>Start from a collection. Here are all six orders.</>,
      <><code>where</code> keeps the rows that pass a test.</>,
      <><code>sort</code> puts the largest total first.</>,
      <><code>limit</code> keeps the first three.</>,
      <><code>select</code> chooses what each row of the answer holds.</>,
    ],
    answer: [`{ id: "o1", total: 125.0 }`, `{ id: "o6", total: 97.0 }`, `{ id: "o2", total: 85.0 }`],
    visual: (stage) => <PipelineVisual stage={stage} />,
  },
  {
    name: "Follow",
    title: "Step through a reference with a dot.",
    code: [
      { from: 0, code: `orders\n` },
      { from: 1, to: 2, code: `  .${k("where")} { .customer.country == ${s("NG")} }\n` },
      { code: `  .${k("sort")} { .id.asc }\n` },
      { from: 3, code: `  .${k("select")} { id, who: .customer.name, total }` },
    ],
    steps: [
      <>Each order holds a reference to one customer.</>,
      <>
        <code>.customer</code> follows it. There is no join to write and no key to match.
      </>,
      <>The test reads a field on the far side. Chen is in SG, so his orders drop out.</>,
      <>The answer can carry fields from the far side too.</>,
    ],
    answer: [
      `{ id: "o1", who: "Ada", total: 125.0 }`,
      `{ id: "o2", who: "Bola", total: 85.0 }`,
      `{ id: "o3", who: "Ada", total: 24.0 }`,
      `{ id: "o6", who: "Ada", total: 97.0 }`,
    ],
    visual: (stage) => <LinksVisual stage={stage} />,
  },
  {
    name: "Turn back",
    title: "Walk the same reference the other way.",
    code: [
      { from: 0, code: `customers.${k("sort")} { .id.asc }.${k("select")} {\n  name,\n` },
      { from: 1, to: 2, code: `  orders: .orders.${k("count")}(),\n` },
      { from: 3, code: `  spent: .orders\n    .${k("where")} { .status == ${s("paid")} }\n    .${k("sum")}(.total)\n` },
      { from: 0, code: `}` },
    ],
    steps: [
      <>Start from customers this time. Nothing on a customer lists their orders.</>,
      <>
        <code>.orders</code> is every order that points here. It behaves like any collection.
      </>,
      <>So it can be counted.</>,
      <>Or filtered and added up, with the same verbs as before.</>,
    ],
    answer: [
      `{ name: "Ada", orders: 3, spent: 222.0 }`,
      `{ name: "Bola", orders: 1, spent: 85.0 }`,
      `{ name: "Chen", orders: 2, spent: 52.0 }`,
    ],
    visual: (stage) => <LinksVisual stage={stage} back />,
  },
  {
    name: "Group",
    title: "Sort rows into piles, then measure each pile.",
    code: [
      { from: 0, code: `orders\n` },
      { from: 1, code: `  .${k("group_by")} { .status }\n` },
      { from: 2, code: `  .${k("select")} {\n    status, n: ${k("count")}(), revenue: ${k("sum")}(.total)\n  }\n` },
      { from: 3, code: `  .${k("sort")} { .revenue.desc }` },
    ],
    steps: [
      <>The same six orders.</>,
      <><code>group_by</code> makes one pile for each status.</>,
      <>Inside <code>select</code>, <code>count()</code> and <code>sum()</code> measure a whole pile.</>,
      <>The piles are rows now, so they sort like rows.</>,
    ],
    answer: [
      `{ status: "paid", n: 4, revenue: 359.0 }`,
      `{ status: "cancelled", n: 1, revenue: 40.0 }`,
      `{ status: "open", n: 1, revenue: 24.0 }`,
    ],
    visual: (stage) => <BucketsVisual stage={stage} />,
  },
  {
    name: "Climb",
    title: "Follow a relationship as far as it goes.",
    code: [
      { from: 0, code: `categories\n  .${k("where")} { .id == ${s("home")} }\n` },
      { from: 1, to: 2, code: `  .${k("walk")}(.children)\n` },
      { from: 3, code: `  .products\n` },
      { code: `  .${k("sort")} { .id.asc }\n` },
      { from: 4, code: `  .${k("select")} { name, price }` },
    ],
    steps: [
      <>Categories form a tree: each one may have a parent. Start at the top.</>,
      <>
        <code>walk</code> follows <code>.children</code> one level down.
      </>,
      <>Then again, until there is nowhere left to go. You never say how deep.</>,
      <>From everything it reached, step across to the products.</>,
      <>Three products, found under a tree two levels deep.</>,
    ],
    answer: [`{ name: "Kettle", price: 40.0 }`, `{ name: "Chef knife", price: 85.0 }`, `{ name: "Trowel", price: 12.0 }`],
    visual: (stage) => <TreeVisual stage={stage} />,
  },
  {
    name: "Place",
    title: "Ask where a row stands among its neighbours.",
    code: [
      { from: 0, code: `orders\n` },
      { from: 1, code: `  .${k("sort")} { .customer.id.asc, .total.desc }\n` },
      { from: 2, code: `  .${k("partition_by")} { .customer }\n` },
      { from: 3, code: `  .${k("select")} {\n    id, who: .customer.name, total,\n    place: ${k("row_number")}()\n  }\n` },
      { from: 4, code: `  .${k("where")} { .place == 1 }` },
    ],
    steps: [
      <>One more time, the six orders.</>,
      <>Sort by customer, largest order first.</>,
      <><code>partition_by</code> lets each customer's orders be counted separately.</>,
      <><code>row_number()</code> is a row's position within its own customer.</>,
      <>Filter on that position: each customer's largest order.</>,
    ],
    answer: [
      `{ id: "o1", who: "Ada", total: 125.0, place: 1 }`,
      `{ id: "o2", who: "Bola", total: 85.0, place: 1 }`,
      `{ id: "o4", who: "Chen", total: 52.0, place: 1 }`,
    ],
    visual: (stage) => <PlaceVisual stage={stage} />,
  },
  {
    name: "Pair",
    title: "Name the rows you pass, and compare them.",
    code: [
      { from: 0, code: `customers {\n` },
      { from: 1, code: `  a ${k("in")} .orders\n` },
      { from: 2, code: `  b ${k("in")} .orders\n` },
      { from: 3, code: `  ${k("where")} { a.total > b.total }\n` },
      { from: 4, code: `  ${k("select")} { who: name, larger: a.id, smaller: b.id }\n` },
      { from: 0, code: `}` },
    ],
    steps: [
      <>Braces after a collection open a block. Each line inside works on its rows: three customers.</>,
      <>
        <code>a in .orders</code> names each order in turn. There is now one row for every customer and order.
      </>,
      <>A second name multiplies again: every pair of one customer's orders.</>,
      <>A test reads each row by its name. Keep the pairs where the first order is the larger.</>,
      <>The answer holds all three rows side by side. No join, and nothing nested.</>,
    ],
    answer: [
      `{ who: "Ada", larger: "o1", smaller: "o3" }`,
      `{ who: "Ada", larger: "o1", smaller: "o6" }`,
      `{ who: "Ada", larger: "o6", smaller: "o3" }`,
      `{ who: "Chen", larger: "o4", smaller: "o5" }`,
    ],
    visual: (stage) => <PairsVisual stage={stage} />,
  },
  {
    name: "Near",
    title: "Find rows by what they are about.",
    code: [
      { from: 0, code: `products\n` },
      { from: 1, code: `  .${k("nearest")} {\n    field: .about,\n` },
      { from: 2, code: `    query: [${SOUGHT.map((n) => n.toFixed(1)).join(", ")}],\n` },
      { from: 3, code: `    k: 2\n` },
      { from: 1, code: `  }\n` },
      { from: 4, code: `  .${k("select")} { name, score: ${k("round")}(._score, 3) }` },
    ],
    steps: [
      <>
        Each product holds a vector: two numbers that say how much it has to do with hot drinks, and with lighting.
      </>,
      <>
        <code>nearest</code> compares a vector with the one in a field of every row. What matters is which way each points.
      </>,
      <>The vector to look for: mostly hot drinks, a little light.</>,
      <>
        <code>k</code> keeps the two that point most nearly the same way. The lamp points elsewhere.
      </>,
      <>
        Each row found carries its score. 1 is exactly the same direction.
      </>,
    ],
    answer: [`{ name: "Mug", score: 0.995 }`, `{ name: "Tea", score: 0.976 }`],
    visual: (stage) => <NearVisual stage={stage} />,
  },
];

const STEP_MS = 2600;

export function Tour() {
  const [chapter, setChapter] = useState(0);
  const [stage, setStage] = useState(0);
  const [playing, setPlaying] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const current = CHAPTERS[chapter];
  const last = current.steps.length - 1;

  // Start playing the first time the tour scrolls into view, unless motion is unwelcome.
  useEffect(() => {
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const seen = new IntersectionObserver(
      ([entry]) => {
        if (!entry.isIntersecting) return;
        setPlaying(true);
        seen.disconnect();
      },
      { threshold: 0.5 },
    );
    seen.observe(box.current!);
    return () => seen.disconnect();
  }, []);

  // While playing, move one step at a time and stop at the end of the chapter.
  useEffect(() => {
    if (!playing) return;
    if (stage >= last) {
      setPlaying(false);
      return;
    }
    const timer = setTimeout(() => setStage((n) => n + 1), STEP_MS);
    return () => clearTimeout(timer);
  }, [playing, stage, last]);

  const open = (n: number) => {
    setChapter(n);
    setStage(0);
    setPlaying(!matchMedia("(prefers-reduced-motion: reduce)").matches);
  };
  const go = (n: number) => {
    setPlaying(false);
    setStage(Math.max(0, Math.min(last, n)));
  };

  return (
    <div className="tour" ref={box}>
      <div className="tour-rail" role="tablist" aria-label="Chapters of the language tour">
        {CHAPTERS.map((c, i) => (
          <button key={c.name} role="tab" aria-selected={i === chapter} onClick={() => open(i)}>
            <b>0{i + 1}</b> {c.name}
          </button>
        ))}
      </div>

      <h3 className="tour-title">{current.title}</h3>
      {CHAPTERS.map((c, i) =>
        i === chapter ? null : (
          <Offstage key={c.name} title={c.title} code={[c.code.map((piece) => piece.code).join("")]} notes={c.steps} answer={c.answer} />
        ),
      )}

      <div className="tour-stage">
        <div className="tour-left">
          <div className="code-card tour-code">
            <pre>
              <code>
                {current.code.map((piece, i) => {
                  const state =
                    piece.from === undefined ? "plain" : stage < piece.from ? "later" : stage <= (piece.to ?? piece.from) ? "now" : "plain";
                  return (
                    <span
                      key={`${chapter}-${i}`}
                      className={`piece ${state}`}
                      onClick={piece.from === undefined ? undefined : () => go(piece.from!)}
                      dangerouslySetInnerHTML={{ __html: piece.code }}
                    />
                  );
                })}
              </code>
            </pre>
          </div>

          <div className="tour-step">
            <p key={`${chapter}-${stage}`} aria-live="polite">
              {current.steps[stage]}
            </p>
            <div className="tour-controls">
              <button className="round" onClick={() => go(stage - 1)} disabled={stage === 0} aria-label="Previous step">
                ←
              </button>
              <div className="dots" aria-hidden="true">
                {current.steps.map((_, i) => (
                  <i key={i} className={i === stage ? "on" : i < stage ? "done" : ""} onClick={() => go(i)} />
                ))}
              </div>
              <button className="round" onClick={() => go(stage + 1)} disabled={stage === last} aria-label="Next step">
                →
              </button>
              {stage === last ? (
                chapter < CHAPTERS.length - 1 ? (
                  <button className="next" onClick={() => open(chapter + 1)}>
                    Next: {CHAPTERS[chapter + 1].name}
                  </button>
                ) : (
                  <button className="next" onClick={() => open(0)}>
                    Start again
                  </button>
                )
              ) : (
                <button className="next quiet" onClick={() => setPlaying((p) => !p)}>
                  {playing ? "Pause" : "Play"}
                </button>
              )}
            </div>
          </div>
        </div>

        <div className="tour-right">
          <div className="tour-visual">{current.visual(stage)}</div>
          {current.answer && (
            <div className={stage === last ? "tour-answer on" : "tour-answer"}>
              <span>Owl answers</span>
              <pre>{current.answer.join("\n")}</pre>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
