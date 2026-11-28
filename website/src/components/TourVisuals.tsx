import type { CSSProperties, ReactNode } from "react";
import { CUSTOMERS, ORDERS, PRODUCTS as SOLD, SOUGHT, countryOf, nameOf } from "../lib/shop";

// Every visual is one 480×400 drawing whose parts move or fade as `stage` advances. Positions
// are computed from the shop's rows with the same filter and sort the query applies, so a
// picture cannot drift from the answer beside it.

type Stage = { stage: number };

const at = (x: number, y: number, opacity = 1): CSSProperties => ({ transform: `translate(${x}px, ${y}px)`, opacity });
const show = (yes: boolean, dim = 0): CSSProperties => ({ opacity: yes ? 1 : dim });

function Frame({ label, children }: { label: string; children: ReactNode }) {
  return (
    <svg className="tv" viewBox="0 0 480 400" role="img" aria-label={label}>
      {children}
    </svg>
  );
}

function Row({ x, y, w, opacity = 1, lit, children }: { x: number; y: number; w: number; opacity?: number; lit?: boolean; children: ReactNode }) {
  return (
    <g className={lit ? "tv-row lit" : "tv-row"} style={at(x, y, opacity)}>
      <rect width={w} height={34} rx={10} style={{ width: w }} />
      {children}
    </g>
  );
}

/* ---------- 01 Describe: the schema becomes two linked cards ---------- */

export function SchemaVisual({ stage }: Stage) {
  return (
    <Frame label="The customers and orders collections, linked by a reference">
      <g className="tv-card" style={at(280, 40, stage >= 0 ? 1 : 0)}>
        <rect width={170} height={stage >= 3 ? 150 : 118} rx={14} style={{ height: stage >= 3 ? 150 : 118 }} />
        <text className="tv-title" x={16} y={28}>customers</text>
        <text x={16} y={58}>id</text>
        <text x={16} y={80}>name</text>
        <text x={16} y={102}>country</text>
        <text className="tv-derived" x={16} y={132} style={show(stage >= 3)}>.orders</text>
        <text className="tv-note" x={160} y={132} textAnchor="end" style={show(stage >= 3)}>derived</text>
      </g>

      <g className="tv-card" style={at(30, 150, stage >= 1 ? 1 : 0)}>
        <rect width={180} height={stage >= 4 ? 218 : 140} rx={14} style={{ height: stage >= 4 ? 218 : 140 }} />
        <text className="tv-title" x={16} y={28}>orders</text>
        <text x={16} y={58}>id</text>
        <text x={16} y={80}>status</text>
        <text x={16} y={102}>total</text>
        <g style={show(stage >= 2)}>
          <rect className="tv-mark" x={8} y={110} width={164} height={22} rx={6} />
          <text x={16} y={126}>customer</text>
        </g>
        <g style={show(stage >= 4)}>
          <text x={16} y={152}>items</text>
          <rect className="tv-inner" x={16} y={162} width={148} height={44} rx={8} />
          <text x={26} y={180}>product  qty</text>
          <text className="tv-note" x={26} y={198}>a list, per order</text>
        </g>
      </g>

      <g className={stage === 2 ? "tv-link walk" : "tv-link"} style={show(stage >= 2)}>
        <path d="M210 270 C 300 270 366 240 366 164" markerEnd="url(#arrow)" />
        <text x={318} y={262}>.customer</text>
      </g>
      <g className={stage === 3 ? "tv-link back walk" : "tv-link back"} style={show(stage >= 3)}>
        <path d="M280 110 C 190 110 120 110 120 144" markerEnd="url(#arrow)" />
        <text x={150} y={96}>.orders</text>
      </g>
    </Frame>
  );
}

/* ---------- 02 Ask: rows pass through where, sort, limit and select ---------- */

export function PipelineVisual({ stage }: Stage) {
  const kept = ORDERS.filter((o) => o.status === "paid").sort((a, b) => b.total - a.total);
  const slim = stage >= 4;
  return (
    <Frame label="Six orders narrowed to the three largest paid ones">
      <g className="tv-head">
        <text x={46} y={30}>id</text>
        <text x={110} y={30} style={show(!slim)}>customer</text>
        <text x={230} y={30} style={show(!slim)}>status</text>
        <text x={slim ? 200 : 426} y={30} textAnchor="end" style={{ transform: "none" }}>total</text>
      </g>
      {ORDERS.map((o, i) => {
        const rank = kept.indexOf(o);
        const paid = rank >= 0;
        const y = 44 + (stage >= 2 && paid ? rank : i) * 44;
        const cut = rank >= 3;
        const opacity = !paid ? (stage >= 2 ? 0 : stage >= 1 ? 0.2 : 1) : cut ? (stage >= 4 ? 0 : stage >= 3 ? 0.2 : 1) : 1;
        return (
          <Row key={o.id} x={30} y={y} w={slim ? 190 : 416} opacity={opacity} lit={stage >= 3 && paid && !cut}>
            <text x={16} y={22}>{o.id}</text>
            <text x={80} y={22} style={show(!slim)}>{nameOf(o.customer)}</text>
            <text x={200} y={22} style={show(!slim)} className={o.status === "paid" && stage === 1 ? "tv-strong" : ""}>{o.status}</text>
            <text x={0} y={22} textAnchor="end" style={{ transform: `translateX(${slim ? 170 : 396}px)` }}>{o.total}</text>
          </Row>
        );
      })}
    </Frame>
  );
}

/* ---------- 03 Follow and 04 Turn back: orders and customers, joined by references ---------- */

const orderY = (i: number) => 24 + i * 58;
const customerY = (j: number) => 52 + j * 116;

export function LinksVisual({ stage, back }: Stage & { back?: boolean }) {
  return (
    <Frame label={back ? "Each customer reaching back to their orders" : "Each order following its reference to a customer"}>
      {ORDERS.map((o, i) => {
        const j = CUSTOMERS.findIndex((c) => c.id === o.customer);
        const out = back ? stage >= 3 && o.status !== "paid" : stage >= 2 && countryOf(o.customer) !== "NG";
        const from = [170, orderY(i) + 17];
        const to = [316, customerY(j) + 17];
        const d = back
          ? `M${to[0]} ${to[1]} C ${to[0] - 70} ${to[1]} ${from[0] + 70} ${from[1]} ${from[0] + 4} ${from[1]}`
          : `M${from[0]} ${from[1]} C ${from[0] + 70} ${from[1]} ${to[0] - 70} ${to[1]} ${to[0] - 4} ${to[1]}`;
        return (
          <g key={o.id}>
            <g className={stage >= 1 && !out ? "tv-link walk" : "tv-link faint"} style={show(!out, 0.12)}>
              <path d={d} markerEnd="url(#arrow)" />
            </g>
            <Row x={24} y={orderY(i)} w={146} opacity={out ? 0.2 : 1} lit={!back && stage >= 3 && !out}>
              <text x={12} y={22}>{o.id}</text>
              <text x={44} y={22} className="tv-strong" style={show(!back && stage >= 3)}>{nameOf(o.customer)}</text>
              <text x={44} y={22} className="tv-note" style={show(!!back && stage >= 3)}>{o.status}</text>
              <text x={134} y={22} textAnchor="end">{o.total}</text>
            </Row>
          </g>
        );
      })}
      {CUSTOMERS.map((c, j) => {
        const mine = ORDERS.filter((o) => o.customer === c.id);
        const spent = mine.filter((o) => o.status === "paid").reduce((sum, o) => sum + o.total, 0);
        const out = !back && stage >= 2 && c.country !== "NG";
        return (
          <g key={c.id} style={at(316, customerY(j), out ? 0.2 : 1)} className={back && stage === 0 ? "tv-row lit" : "tv-row"}>
            <rect width={140} height={back ? 58 : 34} rx={10} style={{ height: back && stage >= 2 ? 58 : 34 }} />
            <text x={12} y={22}>{c.name}</text>
            <text x={128} y={22} textAnchor="end" className="tv-tag" style={show(!back && stage >= 2)}>{c.country}</text>
            <text x={12} y={46} className="tv-note" style={show(!!back && stage >= 2)}>
              orders <tspan className="tv-strong">{mine.length}</tspan>
            </text>
            <text x={128} y={46} textAnchor="end" className="tv-note" style={show(!!back && stage >= 3)}>
              spent <tspan className="tv-strong">{spent}</tspan>
            </text>
          </g>
        );
      })}
    </Frame>
  );
}

/* ---------- 05 Group: orders fall into one pile per status ---------- */

export function BucketsVisual({ stage }: Stage) {
  const statuses = ["paid", "open", "cancelled"] as const;
  const piles = statuses.map((status) => {
    const rows = ORDERS.filter((o) => o.status === status);
    return { status, rows, revenue: rows.reduce((sum, o) => sum + o.total, 0) };
  });
  const sorted = [...piles].sort((a, b) => b.revenue - a.revenue);
  const columnX = (status: string) => 24 + (stage >= 3 ? sorted : piles).findIndex((p) => p.status === status) * 150;
  const most = Math.max(...piles.map((p) => p.revenue));

  return (
    <Frame label="Orders grouped by status, with a count and revenue for each group">
      {piles.map((p) => (
        <g key={p.status} style={at(columnX(p.status), 0, stage >= 1 ? 1 : 0)}>
          <text className="tv-title" x={0} y={92}>{p.status}</text>
          <path className="tv-rule" d="M0 100 H126" />
          <g style={show(stage >= 2)}>
            <text className="tv-note" x={0} y={298}>count() <tspan className="tv-strong">{p.rows.length}</tspan></text>
            <text className="tv-note" x={0} y={320}>sum(.total) <tspan className="tv-strong">{p.revenue}</tspan></text>
            <rect className="tv-bar" x={0} y={332} height={12} rx={6} width={126} style={{ width: stage >= 2 ? Math.max(8, (p.revenue / most) * 126) : 0 }} />
          </g>
        </g>
      ))}
      {ORDERS.map((o, i) => {
        const k = ORDERS.filter((other, n) => other.status === o.status && n < i).length;
        const x = stage >= 1 ? columnX(o.status) : 20 + i * 74;
        const y = stage >= 1 ? 114 + k * 40 : 30;
        return (
          <Row key={o.id} x={x} y={y} w={stage >= 1 ? 126 : 66}>
            <text x={10} y={22}>{o.id}</text>
            <text x={0} y={22} textAnchor="end" style={{ transform: `translateX(${stage >= 1 ? 116 : 58}px)` }}>{o.total}</text>
          </Row>
        );
      })}
    </Frame>
  );
}

/* ---------- 06 Climb: walk a tree to any depth ---------- */

const TREE = [
  { id: "Home", x: 240, y: 30, depth: 0, parent: null },
  { id: "Kitchen", x: 150, y: 118, depth: 1, parent: 0 },
  { id: "Garden", x: 360, y: 118, depth: 1, parent: 0 },
  { id: "Kettles", x: 80, y: 206, depth: 2, parent: 1 },
  { id: "Knives", x: 220, y: 206, depth: 2, parent: 1 },
] as const;
const PRODUCTS = [
  { name: "Kettle", price: 40, under: 3, x: 80 },
  { name: "Chef knife", price: 85, under: 4, x: 220 },
  { name: "Trowel", price: 12, under: 2, x: 360 },
] as const;

export function TreeVisual({ stage }: Stage) {
  return (
    <Frame label="A category tree walked from the top down to its products">
      {[1, 2].map((depth) => (
        <text key={depth} className="tv-note" x={472} y={depth * 88 + 52} textAnchor="end" style={show(stage >= depth)}>
          depth {depth}
        </text>
      ))}
      {TREE.map((n) => {
        if (n.parent === null) return null;
        const p = TREE[n.parent];
        const reached = stage >= n.depth;
        return (
          <g key={n.id} className={reached && stage === n.depth ? "tv-link walk" : reached ? "tv-link" : "tv-link faint"}>
            <path d={`M${p.x} ${p.y + 34} L${n.x} ${n.y - 4}`} markerEnd="url(#arrow)" />
          </g>
        );
      })}
      {PRODUCTS.map((p) => {
        const n = TREE[p.under];
        return (
          <g key={p.name} style={show(stage >= 3)}>
            <g className={stage === 3 ? "tv-link back walk" : "tv-link back"}>
              <path d={`M${n.x} ${n.y + 34} L${p.x} 318`} markerEnd="url(#arrow)" />
            </g>
            <g className="tv-row lit" style={at(p.x - 54, 322)}>
              <rect width={108} height={stage >= 4 ? 52 : 34} rx={10} style={{ height: stage >= 4 ? 52 : 34 }} />
              <text x={54} y={22} textAnchor="middle">{p.name}</text>
              <text x={54} y={42} textAnchor="middle" className="tv-note" style={show(stage >= 4)}>{p.price}</text>
            </g>
          </g>
        );
      })}
      {TREE.map((n) => (
        <g key={n.id} className={stage >= n.depth && stage < 3 ? "tv-row lit" : "tv-row"} style={at(n.x - 48, n.y, stage >= n.depth ? 1 : 0.3)}>
          <rect width={96} height={34} rx={10} />
          <text x={48} y={22} textAnchor="middle">{n.id}</text>
        </g>
      ))}
    </Frame>
  );
}

/* ---------- 07 Place: number rows within each customer ---------- */

export function PlaceVisual({ stage }: Stage) {
  const sorted = [...ORDERS].sort((a, b) => a.customer.localeCompare(b.customer) || b.total - a.total);
  return (
    <Frame label="Orders numbered within each customer, keeping each customer's largest">
      {ORDERS.map((o, i) => {
        const k = sorted.indexOf(o);
        const group = CUSTOMERS.findIndex((c) => c.id === o.customer);
        const place = sorted.filter((other, n) => other.customer === o.customer && n < k).length + 1;
        const y = 30 + (stage >= 1 ? k : i) * 44 + (stage >= 2 ? group * 26 : 0);
        return (
          <Row key={o.id} x={70} y={y} w={340} opacity={stage >= 4 && place > 1 ? 0.2 : 1} lit={stage >= 4 && place === 1}>
            <text x={16} y={22}>{o.id}</text>
            <text x={80} y={22}>{nameOf(o.customer)}</text>
            <text x={250} y={22} textAnchor="end">{o.total}</text>
            <g style={show(stage >= 3)}>
              <circle className="tv-badge" cx={312} cy={17} r={12} />
              <text className="tv-badge-text" x={312} y={22} textAnchor="middle">{place}</text>
            </g>
          </Row>
        );
      })}
      {CUSTOMERS.map((c, group) => {
        const first = sorted.findIndex((o) => o.customer === c.id);
        const size = sorted.filter((o) => o.customer === c.id).length;
        const top = 30 + first * 44 + group * 26;
        return (
          <g key={c.id} style={show(stage >= 2)}>
            <path className="tv-rule strong" d={`M56 ${top} h-10 v${size * 44 - 10} h10`} />
          </g>
        );
      })}
    </Frame>
  );
}

/* ---------- 08 Pair: each customer's orders, paired with one another ---------- */

// Every pair of one customer's orders, in the order the block makes them: customers, then
// the first name's orders, then the second's.
const PAIRS = CUSTOMERS.flatMap((c, group) => {
  const mine = ORDERS.filter((o) => o.customer === c.id);
  return mine.flatMap((a, first) => mine.map((b, second) => ({ c, group, a, b, first, second })));
});

export function PairsVisual({ stage }: Stage) {
  const kept = PAIRS.filter((p) => p.a.total > p.b.total);
  // Before a name is given, the rows it will make stand on one another: a customer's pairs on
  // the customer, then an order's pairs on the order.
  const withOrder = PAIRS.filter((p) => p.second === 0);
  return (
    <Frame label="Each customer's orders paired with one another, keeping the pairs where the first is larger">
      <g className="tv-head">
        <text x={46} y={30}>customer</text>
        <text x={190} y={30} style={show(stage >= 1)}>a</text>
        <text x={320} y={30} style={show(stage >= 2)}>b</text>
      </g>
      {PAIRS.map((p, i) => {
        const passes = kept.includes(p);
        const y =
          stage === 0
            ? 44 + p.group * 40
            : stage === 1
              ? 44 + withOrder.findIndex((q) => q.c === p.c && q.a === p.a) * 40
              : stage >= 4 && passes
                ? 44 + kept.indexOf(p) * 40
                : 44 + i * 25;
        const shown = stage === 0 ? p.first === 0 && p.second === 0 : stage === 1 ? p.second === 0 : true;
        const opacity = !shown ? 0 : passes || stage < 3 ? 1 : stage >= 4 ? 0 : 0.2;
        // Fourteen pairs need slim rows; the few rows before and after them have room.
        const slim = stage === 2 || stage === 3 || (stage >= 4 && !passes);
        const line = slim ? 16 : 22;
        return (
          <g key={`${p.a.id}-${p.b.id}`} className={stage >= 3 && passes ? "tv-row lit" : "tv-row"} style={at(30, y, opacity)}>
            <rect width={416} height={34} rx={8} style={{ height: slim ? 22 : 34 }} />
            <text x={16} y={line}>{p.c.name}</text>
            <g style={show(stage >= 1)}>
              <text x={160} y={line}>{p.a.id}</text>
              <text x={250} y={line} textAnchor="end" className={stage === 3 ? "tv-strong" : "tv-note"}>{p.a.total}</text>
            </g>
            <g style={show(stage >= 2)}>
              <text x={290} y={line}>{p.b.id}</text>
              <text x={380} y={line} textAnchor="end" className={stage === 3 ? "tv-strong" : "tv-note"}>{p.b.total}</text>
            </g>
          </g>
        );
      })}
    </Frame>
  );
}

/* ---------- 09 Near: products as points, and the two that point most nearly the query's way ---------- */

// How nearly two vectors point the same way: the cosine of the angle between them.
const alike = (a: readonly number[], b: readonly number[]) => {
  const dot = a.reduce((sum, n, i) => sum + n * b[i], 0);
  return dot / (Math.hypot(...a) * Math.hypot(...b));
};

// Where a vector lands in the drawing: hot drinks to the right, lighting upwards.
const plot = ([drinks, light]: readonly number[]) => ({ x: 70 + drinks * 300, y: 330 - light * 260 });

// Where each name is written from its point, so that no two names touch: beside the point,
// or under the line the point sits on.
const NAME_AT: Record<string, { dx: number; dy: number; anchor: "start" | "middle" }> = {
  kettle: { dx: 16, dy: 5, anchor: "start" },
  mug: { dx: 16, dy: -2, anchor: "start" },
  tea: { dx: 0, dy: 28, anchor: "middle" },
  lamp: { dx: 16, dy: 5, anchor: "start" },
};

export function NearVisual({ stage }: Stage) {
  const ranked = [...SOLD].sort((a, b) => alike(b.about, SOUGHT) - alike(a.about, SOUGHT));
  const found = ranked.slice(0, 2);
  const origin = plot([0, 0]);
  // The query is a direction, drawn short of the points so that it crosses none of them.
  const sought = plot(SOUGHT.map((n) => n * 0.6));
  return (
    <Frame label="Four products as points, and the two that point most nearly the same way as the vector asked about">
      <path className="tv-rule" d={`M${origin.x} ${origin.y} H 440`} />
      <path className="tv-rule" d={`M${origin.x} ${origin.y} V 40`} />
      <g className="tv-head">
        <text x={origin.x + 8} y={origin.y + 24}>hot drinks</text>
        <text x={origin.x - 8} y={34}>lighting</text>
      </g>

      {SOLD.map((p) => {
        const { x, y } = plot(p.about);
        const lit = found.some((f) => f.id === p.id);
        const name = NAME_AT[p.id];
        return (
          <g key={p.id} style={show(stage < 3 || lit, 0.25)}>
            <path className="tv-link faint" d={`M${origin.x} ${origin.y} L ${x} ${y}`} style={show(stage >= 1)} />
            <circle className={stage >= 3 && lit ? "tv-dot lit" : "tv-dot"} cx={x} cy={y} r={9} />
            <text x={x + name.dx} y={y + name.dy} textAnchor={name.anchor}>{p.name}</text>
            <text className="tv-strong" x={x + name.dx} y={y + name.dy + 17} textAnchor={name.anchor} style={show(stage >= 4 && lit)}>
              {alike(p.about, SOUGHT).toFixed(3)}
            </text>
          </g>
        );
      })}

      <g className={stage === 2 ? "tv-link walk" : "tv-link"} style={show(stage >= 2)}>
        <path d={`M${origin.x} ${origin.y} L ${sought.x} ${sought.y}`} markerEnd="url(#arrow)" />
        <text x={sought.x - 4} y={sought.y + 20} textAnchor="end">query</text>
      </g>
    </Frame>
  );
}
