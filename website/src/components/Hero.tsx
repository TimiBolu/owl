import { useRef, useState } from "react";
import type { CSSProperties, PointerEvent } from "react";

type Say = (text: string, ms?: number) => void;

const START = [
  ["customer", 212, 245], ["order", 130, 407], ["order", 300, 437], ["item", 216, 567],
  ["product", 1401, 233], ["item", 1290, 387], ["review", 1480, 425], ["customer", 1384, 567],
] as const;
const LINKS = [[1, 0], [2, 0], [1, 3], [5, 4], [6, 4], [6, 7]] as const;

/** The chips beside the headline. Each can be dragged, and its links follow. */
function HeroGraph({ say }: { say: Say }) {
  const svg = useRef<SVGSVGElement>(null);
  const [at, setAt] = useState<{ x: number; y: number }[]>(() => START.map(([, x, y]) => ({ x, y })));
  const [held, setHeld] = useState<number | null>(null);
  const drag = useRef<{ i: number; from: { x: number; y: number }; start: DOMPoint } | null>(null);

  const toGraph = (e: PointerEvent) =>
    new DOMPoint(e.clientX, e.clientY).matrixTransform(svg.current!.getScreenCTM()!.inverse());

  const down = (i: number) => (e: PointerEvent<SVGGElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { i, from: at[i], start: toGraph(e) };
    setHeld(i);
  };
  const move = (e: PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const p = toGraph(e);
    setAt((all) => all.map((n, i) => (i === d.i ? { x: d.from.x + p.x - d.start.x, y: d.from.y + p.y - d.start.y } : n)));
  };
  const drop = () => {
    const d = drag.current;
    if (!d) return;
    if (Math.hypot(at[d.i].x - d.from.x, at[d.i].y - d.from.y) > 60) say("still linked.");
    drag.current = null;
    setHeld(null);
  };

  return (
    <svg ref={svg} className="hero-graph" viewBox="0 0 1600 900" preserveAspectRatio="xMidYMid meet" aria-hidden="true">
      <g className="links">
        {LINKS.map(([a, b], i) => (
          <line key={i} x1={at[a].x} y1={at[a].y} x2={at[b].x} y2={at[b].y} />
        ))}
      </g>
      {START.map(([label], i) => {
        const w = label.length * 10 + 44;
        return (
          <g
            key={i}
            className={held === i ? "n held" : "n"}
            transform={`translate(${at[i].x} ${at[i].y})`}
            onPointerDown={down(i)}
            onPointerMove={move}
            onPointerUp={drop}
            onPointerCancel={drop}
          >
            <g className="bob" style={{ "--d": `${-i * 0.9}s` } as CSSProperties}>
              <rect x={-w / 2} y={-19} width={w} height={38} rx={19} />
              <text y={6}>{label}</text>
            </g>
          </g>
        );
      })}
    </svg>
  );
}

export function Owl({ hoot }: { hoot: () => void }) {
  return (
    <svg className="owl" id="owl" viewBox="0 0 240 300" role="img" aria-label="An owl whose eyes follow your cursor">
      <g filter="url(#rough-fine)">
        <path className="branch" d="M-260 266 C-80 248 110 276 240 260 S 420 250 520 262" />
        <path className="twig" d="M300 259 l46 -34 M-120 258 l-34 -30" />
        <g className="bird" onClick={hoot}>
          <path className="ink" d="M62 72 L48 20 L98 50Z M178 72 L192 20 L142 50Z" />
          <path className="ink" d="M120 40 C60 40 44 95 46 150 C48 215 80 258 120 258 C160 258 192 215 194 150 C196 95 180 40 120 40Z" />
          <path className="feather" d="M60 150 C58 200 78 234 100 249 M180 150 C182 200 162 234 140 249" />
          <path className="feather" d="M98 178 q11 11 22 0 q11 11 22 0 M109 200 q11 11 22 0 M98 222 q11 11 22 0 q11 11 22 0" />
          <g className="eyes">
            <g className="eye">
              <circle cx="90" cy="108" r="31" />
              <circle className="pupil" cx="90" cy="108" r="13" />
            </g>
            <g className="eye">
              <circle cx="150" cy="108" r="31" />
              <circle className="pupil" cx="150" cy="108" r="13" />
            </g>
          </g>
          <path className="beak" d="M120 124 L109 139 L120 153 L131 139Z" />
          <path className="feet" d="M96 256 v14 M104 256 v15 M112 256 v14 M128 256 v14 M136 256 v15 M144 256 v14" />
        </g>
      </g>
    </svg>
  );
}

export function Hero({ line, say, hoot }: { line: string | null; say: Say; hoot: () => void }) {
  // Keep the last words in the bubble while it shrinks away.
  const last = useRef("");
  if (line) last.current = line;

  return (
    <section className="hero">
      <svg className="hills" viewBox="0 0 1600 500" preserveAspectRatio="none" aria-hidden="true">
        <path d="M0 260 L140 210 L300 250 L470 180 L640 240 L820 200 L1000 150 L1180 215 L1360 170 L1600 230 L1600 500 L0 500Z" fill="#14120f" opacity=".07" />
        <path d="M0 320 L200 270 L380 310 L560 260 L760 330 L940 280 L1150 325 L1330 265 L1600 315 L1600 500 L0 500Z" fill="#14120f" opacity=".09" />
        <path d="M0 400 C300 370 520 420 800 392 C1100 362 1340 418 1600 386 L1600 500 L0 500Z" fill="#14120f" opacity=".06" />
      </svg>

      <HeroGraph say={say} />

      <h1 className="cut">
        <span className="cut-line l1">Your data is</span>
        <span className="cut-line l2">already a graph.</span>
        <span className="cut-line l3">Query it that way.</span>
      </h1>

      <p className="lede">
        Owl is a database for objects that point at each other.
        <br className="wide" /> You follow the references. You never write a join.
      </p>

      <div className="path-chip" aria-label="orders dot customer dot name">
        <span>orders</span>
        <i>.</i>
        <span>customer</span>
        <i>.</i>
        <span>name</span>
      </div>

      <div className="cta">
        <a className="btn solid" href="#language">
          See the language
        </a>
        <a className="btn" href="#start">
          Run it
        </a>
      </div>

      <div className="perch">
        <div className={line ? "bubble on" : "bubble"} role="status">
          {last.current}
        </div>
        <p className="hint">
          <kbd>H</kbd> to hoot
        </p>
        <Owl hoot={hoot} />
      </div>
    </section>
  );
}
