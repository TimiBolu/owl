import { Offstage } from "./Offstage";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { CUSTOMERS, ORDERS } from "../lib/shop";

// Every function here was defined on a real owl-server holding the rows in src/lib/shop.ts, and
// every answer shown is what the server returned. The definitions were still there after the
// server was restarted, and defining `tier` again changed the answers that use it.

const k = (word: string) => `<span class="k">${word}</span>`;
const s = (text: string) => `<span class="s">"${text}"</span>`;
const f = (name: string) => `<span class="fn">${name}</span>`;

/* ---------- The machine: things go in on the left and come out on the right ---------- */

type Item = { put: string; out: string[] | null };

/**
 * `items` go in one at a time. Each comes out as zero or more chips, or is turned away
 * (`out: null`). With `collect`, nothing comes out until everything has gone in, and then
 * one chip does.
 */
function Machine({ name, items, collect }: { name: string; items: Item[]; collect?: string }) {
  const [tick, setTick] = useState(0);
  const [live, setLive] = useState(false);
  const box = useRef<SVGSVGElement>(null);
  const done = tick > items.length - 1 + (collect ? 1 : 0);

  useEffect(() => {
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setTick(items.length + 1);
      return;
    }
    const seen = new IntersectionObserver(([entry]) => setLive(entry.isIntersecting), { threshold: 0.4 });
    seen.observe(box.current!);
    return () => seen.disconnect();
  }, [items.length]);

  useEffect(() => {
    if (!live) return;
    const timer = setTimeout(() => setTick((t) => (t > items.length + 1 ? 0 : t + 1)), done ? 2600 : 950);
    return () => clearTimeout(timer);
  }, [live, tick, done, items.length]);

  const inY = (i: number) => 165 - (items.length * 38) / 2 + i * 38;
  const outputs = collect ? [collect] : items.flatMap((item) => item.out ?? []);
  const outY = (n: number) => 165 - (outputs.length * 38) / 2 + n * 38;
  let produced = 0;
  let refused = 0;

  return (
    <svg ref={box} className="tv machine" viewBox="0 0 480 330" role="img" aria-label={`The ${name} function turning what goes in into what comes out`}>
      {items.map((item, i) => {
        const gone = tick > i;
        return (
          <g key={`in${i}`} className="tv-row" style={{ transform: `translate(${gone ? 190 : 16}px, ${gone ? 148 : inY(i)}px)`, opacity: gone ? 0 : 1 }}>
            <rect width={104} height={32} rx={10} />
            <text x={52} y={21} textAnchor="middle">{item.put}</text>
          </g>
        );
      })}

      {items.map((item, i) => {
        const gone = tick > i;
        if (item.out === null) {
          const slot = refused++;
          return (
            <g key={`no${i}`} className="tv-row refused" style={{ transform: `translate(${gone ? 96 + slot * 74 : 204}px, ${gone ? 286 : 180}px)`, opacity: gone ? 0.5 : 0 }}>
              <rect width={68} height={28} rx={9} />
              <text x={34} y={19} textAnchor="middle">{item.put.split(" ")[0]}</text>
            </g>
          );
        }
        if (collect) return null;
        return item.out.map((text) => {
          const slot = produced++;
          return (
            <g key={`out${i}${text}`} className="tv-row lit" style={{ transform: `translate(${gone ? 360 : 204}px, ${gone ? outY(slot) : 148}px)`, opacity: gone ? 1 : 0 }}>
              <rect width={104} height={32} rx={10} />
              <text x={52} y={21} textAnchor="middle">{text}</text>
            </g>
          );
        });
      })}
      {collect && (
        <g className="tv-row lit" style={{ transform: `translate(${tick > items.length ? 360 : 204}px, 148px)`, opacity: tick > items.length ? 1 : 0 }}>
          <rect width={104} height={32} rx={10} />
          <text x={52} y={21} textAnchor="middle">{collect}</text>
        </g>
      )}

      {/* The machine sits on top, so chips change inside it out of sight. */}
      <g className={done ? "machine-body" : "machine-body busy"}>
        <path className="mouth" d="M150 132 L186 146 L186 184 L150 198Z" />
        <path className="mouth" d="M330 146 L294 150 L294 180 L330 184Z" />
        <rect className="shell" x={182} y={96} width={116} height={138} rx={22} />
        <circle className="eye" cx={219} cy={146} r={15} />
        <circle className="eye" cx={261} cy={146} r={15} />
        <circle className="pupil" cx={222} cy={147} r={6} />
        <circle className="pupil" cx={264} cy={147} r={6} />
        <path className="beak" d="M240 160 l-7 9 l7 9 l7 -9Z" />
        <text className="machine-name" x={240} y={214} textAnchor="middle">{name}()</text>
      </g>
      <text className="tv-note" x={68} y={24} textAnchor="middle">goes in</text>
      <text className="tv-note" x={412} y={24} textAnchor="middle">comes out</text>
      {refused > 0 && <text className="tv-note" x={16} y={305}>turned away</text>}
    </svg>
  );
}

/* ---------- Five kinds of function ---------- */

const paidTotal = (customer: string) =>
  ORDERS.filter((o) => o.customer === customer && o.status === "paid").reduce((sum, o) => sum + o.total, 0);

type Kind = {
  name: string;
  title: string;
  about: ReactNode;
  define: string;
  use: string;
  answer: string[];
  goodFor: string[];
  machine: ReactNode;
};

const KINDS: Kind[] = [
  {
    name: "Measure",
    title: "A row goes in. A number comes out.",
    about: (
      <>
        <code>*</code> hands the function the whole current row. Inside, <code>c.orders</code> walks the reference
        back, the same as anywhere else in OQL.
      </>
    ),
    define: `${k("fn")} ${f("spent")}(c: customers) {\n  c.orders\n    .${k("where")} { .status == ${s("paid")} }\n    .${k("sum")}(.total)\n}`,
    use: `customers.${k("sort")} { .id.asc }.${k("select")} {\n  name,\n  spent: ${f("spent")}(*)\n}`,
    answer: [`{ name: "Ada", spent: 222.0 }`, `{ name: "Bola", spent: 85.0 }`, `{ name: "Chen", spent: 52.0 }`],
    goodFor: ["lifetime spend", "open balance", "days since the last order"],
    machine: <Machine name="spent" items={CUSTOMERS.map((c) => ({ put: c.name, out: [String(paidTotal(c.id))] }))} />,
  },
  {
    name: "Label",
    title: "A value goes in. A word comes out.",
    about: (
      <>
        Functions call each other. Here <code>tier</code> takes what <code>spent</code> worked out, so the rule for a
        gold customer is written in one place.
      </>
    ),
    define: `${k("fn")} ${f("tier")}(spent) {\n  ${k("if")} spent >= 200 ${k("then")} ${s("gold")}\n  ${k("else")} ${k("if")} spent >= 80 ${k("then")} ${s("silver")}\n  ${k("else")} ${s("bronze")}\n}`,
    use: `customers.${k("sort")} { .id.asc }.${k("select")} {\n  name,\n  spent: ${f("spent")}(*),\n  tier: ${f("tier")}(${f("spent")}(*))\n}`,
    answer: [
      `{ name: "Ada", spent: 222.0, tier: "gold" }`,
      `{ name: "Bola", spent: 85.0, tier: "silver" }`,
      `{ name: "Chen", spent: 52.0, tier: "bronze" }`,
    ],
    goodFor: ["pricing tiers", "risk bands", "statuses people can read"],
    machine: (
      <Machine
        name="tier"
        items={[
          { put: "222", out: ["gold"] },
          { put: "85", out: ["silver"] },
          { put: "52", out: ["bronze"] },
        ]}
      />
    ),
  },
  {
    name: "Gate",
    title: "A row goes in. Yes or no comes out.",
    about: (
      <>
        A test with a name. <code>where</code> keeps the rows it says yes to, and the query reads like the question
        you asked.
      </>
    ),
    define: `${k("fn")} ${f("is_big")}(o: orders) -> Bool {\n  o.status == ${s("paid")} && o.total >= 90\n}`,
    use: `orders\n  .${k("where")} { ${f("is_big")}(*) }\n  .${k("sort")} { .id.asc }\n  .${k("select")} { id, total }`,
    answer: [`{ id: "o1", total: 125.0 }`, `{ id: "o6", total: 97.0 }`],
    goodFor: ["who to call today", "what counts as active", "orders that need a second look"],
    machine: (
      <Machine
        name="is_big"
        items={ORDERS.map((o) => ({ put: `${o.id} ${o.total}`, out: o.status === "paid" && o.total >= 90 ? [`${o.id} ${o.total}`] : null }))}
      />
    ),
  },
  {
    name: "Summarise",
    title: "Many values go in. One comes out.",
    about: (
      <>
        Your own way to boil a list down. It can be called three ways, and all three mean the same:{" "}
        <code>.orders.total.spread()</code>, <code>spread(.orders.total)</code>, or on the end of a query.
      </>
    ),
    define: `${k("fn")} ${f("spread")}(values) {\n  values.${k("max")}() - values.${k("min")}()\n}`,
    use: `customers.${k("sort")} { .id.asc }.${k("select")} {\n  name,\n  spread: .orders.total.${f("spread")}()\n}`,
    answer: [`{ name: "Ada", spread: 101.0 }`, `{ name: "Bola", spread: 0.0 }`, `{ name: "Chen", spread: 12.0 }`],
    goodFor: ["how far prices spread", "a score of your own", "a statistic Owl does not ship"],
    machine: (
      <Machine
        name="spread"
        collect="101"
        items={ORDERS.filter((o) => o.customer === "ada").map((o) => ({ put: String(o.total), out: [] }))}
      />
    ),
  },
  {
    name: "Step",
    title: "A row goes in. Related rows come out.",
    about: (
      <>
        A function can return a whole collection. Then it chains like any built-in step, and you have added a path to
        your data that the schema never declared.
      </>
    ),
    define: `${k("fn")} ${f("also_ordered")}(o: orders) {\n  orders.${k("where")} {\n    .customer == o.customer && .id != o.id\n  }\n}`,
    use: `orders\n  .${k("where")} { .id == ${s("o1")} }\n  .${f("also_ordered")}()\n  .${k("sort")} { .id.asc }\n  .${k("select")} { id, total }`,
    answer: [`{ id: "o3", total: 24.0 }`, `{ id: "o6", total: 97.0 }`],
    goodFor: ["a customer's other orders", "people at the same level", "things bought together"],
    machine: <Machine name="also_ordered" items={[{ put: "o1 125", out: ["o3 24", "o6 97"] }]} />,
  },
];

function Kinds() {
  const [at, setAt] = useState(0);
  const kind = KINDS[at];
  return (
    <div className="kinds">
      <div className="tour-rail" role="tablist" aria-label="Kinds of function">
        {KINDS.map((item, i) => (
          <button key={item.name} role="tab" aria-selected={i === at} onClick={() => setAt(i)}>
            <b>0{i + 1}</b> {item.name}
          </button>
        ))}
      </div>
      <h3 className="tour-title">{kind.title}</h3>
      {KINDS.map((other, i) =>
        i === at ? null : (
          <Offstage
            key={other.name}
            title={other.title}
            code={[other.define, other.use]}
            notes={[other.about, `Good for: ${other.goodFor.join(", ")}.`]}
            answer={other.answer}
          />
        ),
      )}
      <div className="kinds-stage">
        <div className="kinds-left">
          <div className="code-card form-code">
            <span className="form-tag">Define it once</span>
            <pre>
              <code dangerouslySetInnerHTML={{ __html: kind.define }} />
            </pre>
          </div>
          <div className="code-card form-code">
            <span className="form-tag">Use it</span>
            <pre>
              <code dangerouslySetInnerHTML={{ __html: kind.use }} />
            </pre>
          </div>
        </div>
        <div className="kinds-right">
          <div className="tour-visual" key={kind.name}>
            {kind.machine}
          </div>
          <div className="tour-answer on">
            <span>Owl answers</span>
            <pre>{kind.answer.join("\n")}</pre>
          </div>
        </div>
      </div>
      <p className="kinds-about" key={at}>
        {kind.about}
      </p>
      <div className="good-for">
        <span>Good for</span>
        {kind.goodFor.map((use) => (
          <i key={use}>{use}</i>
        ))}
      </div>
    </div>
  );
}

/* ---------- One definition, every caller ---------- */

const tierOf = (spent: number, gold: number) => (spent >= gold ? "gold" : spent >= 80 ? "silver" : "bronze");

function OneRule() {
  const [gold, setGold] = useState(200);
  const people = CUSTOMERS.map((c) => ({ name: c.name, spent: paidTotal(c.id), tier: tierOf(paidTotal(c.id), gold) }));
  const count = (tier: string) => people.filter((p) => p.tier === tier).length;

  return (
    <div className="one-rule">
      <div className="code-card form-code rule-code">
        <span className="form-tag">In the database</span>
        <pre>
          <code>
            <span dangerouslySetInnerHTML={{ __html: `${k("fn")} ${f("tier")}(spent) {\n  ${k("if")} spent >= ` }} />
            <span className="dial" key={gold}>{gold}</span>
            <span
              dangerouslySetInnerHTML={{
                __html: ` ${k("then")} ${s("gold")}\n  ${k("else")} ${k("if")} spent >= 80 ${k("then")} ${s("silver")}\n  ${k("else")} ${s("bronze")}\n}`,
              }}
            />
          </code>
        </pre>
        <label className="slider">
          <span>Gold starts at {gold}</span>
          <input type="range" min={80} max={250} step={10} value={gold} onChange={(e) => setGold(Number(e.target.value))} />
        </label>
      </div>

      <div className="callers">
        <article>
          <h4>A dashboard</h4>
          {people.map((p) => (
            <p key={p.name} className="caller-row">
              {p.name} <span className={`badge ${p.tier}`} key={p.tier}>{p.tier}</span>
            </p>
          ))}
        </article>
        <article>
          <h4>The checkout service</h4>
          <pre>{people.map((p) => `{ "who": "${p.name}",\n  "tier": "${p.tier}" }`).join("\n")}</pre>
        </article>
        <article>
          <h4>The monthly report</h4>
          {["gold", "silver", "bronze"].map((tier) => (
            <p key={tier} className="caller-row">
              <span className={`badge ${tier}`}>{tier}</span>
              <b key={count(tier)}>{count(tier)}</b>
            </p>
          ))}
        </article>
      </div>
      <p className="one-rule-note">
        Move the slider. This page works the rule out itself to show the effect. On a real server, defining{" "}
        <code>tier</code> again with 250 turned Ada from gold to silver in every query that used it.
      </p>
    </div>
  );
}

/* ---------- The toolbox ---------- */

const TOOLBOX: [string, string[]][] = [
  ["Text", ["trim", "upper", "lower", "length", "substring", "concat", "replace", "split", "starts_with", "ends_with", "contains", "matches", "pad_left", "pad_right"]],
  ["Numbers", ["abs", "round", "floor", "ceil", "sqrt", "pow", "min", "max", "log", "ln", "exp", "random"]],
  ["Dates", ["now", "today", "days_ago", "hours_ago", "format_date", "parse_date", "year", "month", "day", "weekday", "add_days", "diff_days", "days_until", "days_since"]],
  ["Values", ["coalesce", "if_null", "to_string", "to_int", "to_float", "to_bool", "type_of"]],
  ["Written in OQL itself", ["variance", "stddev", "variance_pop", "stddev_pop", "mode"]],
];

export function Functions() {
  return (
    <section className="band" id="functions">
      <div className="wrap">
        <p className="eyebrow">Functions</p>
        <h2>
          Teach Owl
          <br />
          your own words.
        </h2>
        <p className="sub">
          A function gives a name to a measure, a rule, a test or a path. You write it in OQL, Owl keeps it in the
          database, and from then on every query can use the word.
        </p>

        <Kinds />

        <h3 className="forms-h">One definition, every caller</h3>
        <p className="forms-p">
          A function lives in the database, not in each program that talks to it. Change the rule once and every
          caller gets the new answer.
        </p>
        <OneRule />
        <div className="kept">
          <p>
            <b>Kept with your data.</b> A definition is written to the database like any other row.
          </p>
          <p>
            <b>Still there tomorrow.</b> It survives a restart.
          </p>
          <p>
            <b>Replaced by saying it again.</b> The latest definition is the one Owl uses.
          </p>
          <p>
            <b>Undone with its transaction.</b> Roll the transaction back and the old definition returns.
          </p>
        </div>

        <h3 className="forms-h">What comes in the box</h3>
        <p className="forms-p">
          The last row is written in OQL, the same way yours are. Define one of the same name and yours takes its
          place.
        </p>
        <div className="toolbox">
          {TOOLBOX.map(([group, names]) => (
            <div key={group}>
              <h4>{group}</h4>
              <p>
                {names.map((name) => (
                  <code key={name}>{name}</code>
                ))}
              </p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
