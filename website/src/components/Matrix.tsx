import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import areas from "../lib/matrix.json";
import { GITHUB_URL } from "../lib/seo";
import { Footer, InkFilters } from "./Sections";

type Answer = "yes" | "different" | "no";
type Row = (typeof areas)[number]["rows"][number] & { answer: Answer };
type Feature = Row & { area: string; index: number };

// Every feature in the order the matrix lists them, with the area it is under.
const FEATURES: Feature[] = areas.flatMap(({ area, rows }) => rows.map((row) => ({ ...(row as Row), area }))).map(
  (row, index) => ({ ...row, index }),
);
const COUNT = { yes: 0, different: 0, no: 0 };
for (const feature of FEATURES) COUNT[feature.answer] += 1;

const WORD: Record<Answer, string> = { yes: "Yes", different: "Another way", no: "No" };
const MARK: Record<Answer, string> = { yes: "✓", different: "~", no: "✗" };

const slug = (area: string) => `area-${area.toLowerCase().replace(/[^a-z]+/g, "-")}`;

// The tour visits every window once before repeating, far enough apart each time to look unplanned.
const STRIDE = (() => {
  const coprime = (a: number, b: number): boolean => (b === 0 ? a === 1 : coprime(b, a % b));
  let step = Math.max(1, Math.round(FEATURES.length * 0.38));
  while (!coprime(step, FEATURES.length)) step += 1;
  return step;
})();

/** Words in brackets in the matrix are a remark, not code. */
/** For each character of a query, how deep in brackets it sits, or -1 inside a string. */
function depths(text: string): number[] {
  const out: number[] = [];
  let depth = 0;
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      out.push(-1);
      if (c === "\\") out.push(-1), i++;
      else if (c === '"') quoted = false;
      continue;
    }
    if (c === '"') quoted = true;
    if (")}]".includes(c)) depth--;
    out.push(c === '"' ? -1 : depth);
    if ("({[".includes(c)) depth++;
  }
  return out;
}

/** Cut `text` before each index in `at`. */
const cut = (text: string, at: number[]) =>
  [0, ...at].map((from, i) => text.slice(from, at[i] ?? text.length).trim());

/** Put `parts` on as few lines as hold them; one too long for a line is laid out on its own. */
function pack(parts: string[], pad: string, width: number): string[] {
  const lines: string[] = [];
  let line = "";
  for (const part of parts) {
    if (line && pad.length + line.length + 1 + part.length <= width) {
      line += " " + part;
      continue;
    }
    if (line) lines.push(pad + line);
    line = "";
    if (pad.length + part.length <= width) line = part;
    else lines.push(...layOut(part, pad, width));
  }
  if (line) lines.push(pad + line);
  return lines;
}

/**
 * Lay one OQL expression out over lines no wider than `width`, where it can
 * be: a chain breaks before each step, a condition before `&&`, `||` and
 * `else`, and a body too long for its line goes on lines of its own between
 * its brackets, as many parts to a line as fit.
 */
function layOut(text: string, pad: string, width: number): string[] {
  if (pad.length + text.length <= width) return [pad + text];
  const depth = depths(text);
  const top = (i: number) => depth[i] === 0;

  const steps: number[] = [];
  for (let i = 1; i < text.length - 1; i++) {
    if (text[i] === "." && top(i) && "})".includes(text[i - 1]) && /[a-z_]/i.test(text[i + 1])) steps.push(i);
  }
  if (steps.length) {
    const lines: string[] = [];
    cut(text, steps).forEach((step, i) => {
      // A step after one that closed on a line of its own carries on from
      // the closing bracket: `}.select { id }`.
      const last = lines[lines.length - 1];
      if (i && /^\s*[)}\]]$/.test(last)) {
        const [first, ...rest] = layOut(step, pad, width - 1);
        lines[lines.length - 1] = last + first.trim();
        lines.push(...rest);
      } else {
        lines.push(...layOut(step, i ? pad + "  " : pad, width));
      }
    });
    return lines;
  }

  const turns = [...text].flatMap((_, i) =>
    i > 0 && top(i) && text[i - 1] === " " && ["&& ", "|| ", "else "].some((word) => text.startsWith(word, i))
      ? [i]
      : [],
  );
  if (turns.length) {
    const [first, ...rest] = cut(text, turns);
    const deeper = rest.some((part) => part.startsWith("else ")) ? pad + "  " : pad;
    return [...layOut(first, pad, width), ...pack(rest, deeper, width)];
  }

  const open = [...text].findIndex((c, i) => top(i) && "({[".includes(c));
  if (open < 0) return [pad + text];
  let close = open + 1;
  while (close < text.length && !(top(close) && ")}]".includes(text[close]))) close++;
  const body = text.slice(open + 1, close).trim();
  if (!body) return [pad + text];
  const inner = depths(body);
  const commas = [...body].flatMap((c, i) => (c === "," && inner[i] === 0 ? [i + 1] : []));
  return [
    pad + text.slice(0, open + 1),
    ...pack(cut(body, commas), pad + "  ", width),
    pad + text.slice(close),
  ];
}

/** A query or script as it is run, laid out to be read in a narrow column. */
function laidOut(source: string, width = 45): string {
  return source
    .split("\n")
    .flatMap((line) => {
      const pad = /^\s*/.exec(line)![0];
      return layOut(line.trim(), pad, width);
    })
    .join("\n");
}

function Spoken({ text, block }: { text: string; block?: boolean }) {
  const remark = /^\((.*)\)$/s.exec(text);
  if (remark) return <em className="mx-remark">{remark[1][0].toUpperCase() + remark[1].slice(1)}.</em>;
  return block ? (
    <pre className="mx-code">
      <code>{text}</code>
    </pre>
  ) : (
    <code>{text}</code>
  );
}

/** A note from the matrix, with `code` in it set as code. */
function Note({ text }: { text: string }) {
  return (
    <>
      {text.split("`").map((part, i) => (i % 2 ? <code key={i}>{part}</code> : part))}
    </>
  );
}

/** The rows a query gave, one to a line. */
const rowsOf = (gives: string) => gives.replace(/\} \{/g, "}\n{");

/** What Owl says for a feature: the query, what it gave, what it refuses, and anything to add. */
function OwlSide({ feature }: { feature: Feature }) {
  const parts: ReactNode[] = [];
  if (feature.oql) parts.push(<Spoken key="oql" text={laidOut(feature.oql)} block />);
  if (feature.check) {
    parts.push(
      <p key="then" className="mx-then">
        then
      </p>,
      <Spoken key="check" text={laidOut(feature.check)} block />,
    );
  }
  if (feature.gives) {
    parts.push(
      <pre key="gives" className="mx-gives" aria-label="What it gave">
        <code>{rowsOf(feature.gives)}</code>
      </pre>,
    );
  }
  if (feature.test) {
    const [name, file] = feature.test.split(" in ");
    parts.push(
      <p key="test" className="mx-note">
        Held by the test <code>{name}</code> in <code>{file}</code>.
      </p>,
    );
  }
  if (feature.note) {
    parts.push(
      <p key="note" className="mx-note">
        <Note text={feature.note} />
      </p>,
    );
  }
  if (feature.refuses) {
    parts.push(
      <p key="refused" className="mx-then">
        refused
      </p>,
      <pre key="refuses" className="mx-code mx-refused">
        <code>{laidOut(feature.refuses)}</code>
      </pre>,
    );
  }
  return <>{parts}</>;
}

/** Counts up to a number once it is on screen. */
function CountUp({ to }: { to: number }) {
  const [shown, setShown] = useState(to);
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    setShown(0);
    let frame = 0;
    const seen = new IntersectionObserver(([entry]) => {
      if (!entry.isIntersecting) return;
      seen.disconnect();
      const start = performance.now();
      const tick = (now: number) => {
        const t = Math.min(1, (now - start) / 1100);
        setShown(Math.round(to * (1 - (1 - t) ** 3)));
        if (t < 1) frame = requestAnimationFrame(tick);
      };
      frame = requestAnimationFrame(tick);
    });
    seen.observe(el);
    return () => {
      seen.disconnect();
      cancelAnimationFrame(frame);
    };
  }, [to]);
  return <span ref={ref}>{shown}</span>;
}

/** One building an area, one window a feature: lit for yes, half lit for another way, dark for no. */
function Skyline({ picked, pick }: { picked: number; pick: (index: number) => void }) {
  let index = 0;
  return (
    <div className="mx-skyline" role="group" aria-label="Every feature in the matrix, as a window">
      {areas.map(({ area, rows }, building) => (
        <div className="mx-building" key={area} style={{ "--b": building } as CSSProperties}>
          <div className="mx-windows">
            {rows.map((row) => {
              const mine = index++;
              return (
                <button
                  key={row.feature}
                  type="button"
                  className={`mx-window ${row.answer}${mine === picked ? " picked" : ""}`}
                  style={{ "--i": mine } as CSSProperties}
                  aria-label={`${row.feature}: ${WORD[row.answer as Answer]}`}
                  aria-pressed={mine === picked}
                  onClick={() => pick(mine)}
                  onMouseEnter={() => pick(mine)}
                  onFocus={() => pick(mine)}
                >
                  {row.answer === "no" && <i aria-hidden="true" />}
                </button>
              );
            })}
          </div>
          <a className="mx-plate" href={`#${slug(area)}`} title={area}>
            {area.split(" ")[0].toLowerCase()}
          </a>
        </div>
      ))}
    </div>
  );
}

/** The feature behind the window that is picked, in all three languages. */
function Reader({ feature }: { feature: Feature }) {
  return (
    <article className={`mx-reader ${feature.answer}`} key={feature.index} aria-live="polite">
      <header>
        <p className="mx-where">{feature.area}</p>
        <h3>{feature.feature}</h3>
        <span className="mx-stamp">{WORD[feature.answer]}</span>
      </header>
      <div className="mx-tongues">
        <section>
          <h4>SQL</h4>
          <Spoken text={feature.sql} block />
        </section>
        <section>
          <h4>MongoDB</h4>
          <Spoken text={feature.mongo} block />
        </section>
        <section className="owl-side">
          <h4>Owl</h4>
          <OwlSide feature={feature} />
        </section>
      </div>
    </article>
  );
}

const FILTERS: [Answer | "all", string][] = [
  ["all", "All"],
  ["yes", "Yes"],
  ["different", "Another way"],
  ["no", "No"],
];

/** Every row of the matrix, by area, to filter and open. */
function Ledger() {
  const [only, setOnly] = useState<Answer | "all">("all");
  const [words, setWords] = useState("");
  const wanted = words.trim().toLowerCase();
  const keeps = (feature: Feature) =>
    (only === "all" || feature.answer === only) &&
    (!wanted ||
      [feature.feature, feature.sql, feature.mongo, feature.oql, feature.note, feature.refuses]
        .join("\n")
        .toLowerCase()
        .includes(wanted));
  const shown = useMemo(() => FEATURES.filter(keeps), [only, wanted]);
  const total = (answer: Answer | "all") => (answer === "all" ? FEATURES.length : COUNT[answer]);

  // Each area rises, and its meter fills, when it comes into view.
  useEffect(() => {
    const seen = new IntersectionObserver(
      (entries) =>
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          entry.target.classList.add("in");
          seen.unobserve(entry.target);
        }),
      { rootMargin: "0px 0px -8% 0px" },
    );
    document.querySelectorAll(".mx-area").forEach((el) => {
      el.classList.add("rise");
      seen.observe(el);
    });
    return () => seen.disconnect();
  }, []);

  return (
    <section className="band mx-ledger" id="list">
      <div className="wrap">
        <p className="eyebrow">The whole list</p>
        <h2>
          Every row,
          <br />
          side by side.
        </h2>
        <p className="sub">
          Open any row to see how SQL says it, how MongoDB says it, and the OQL the test runs with the answer it has to
          give.
        </p>

        <div className="mx-tools">
          <div className="mx-chips" role="group" aria-label="Show">
            {FILTERS.map(([answer, label]) => (
              <button
                key={answer}
                type="button"
                className={`mx-chip ${answer}`}
                aria-pressed={only === answer}
                onClick={() => setOnly(answer)}
              >
                {label} <b>{total(answer)}</b>
              </button>
            ))}
          </div>
          <input
            className="mx-find"
            type="search"
            placeholder="Find: join, $unwind, upsert…"
            aria-label="Find a feature"
            value={words}
            onChange={(event) => setWords(event.target.value)}
          />
        </div>
        <p className="mx-shown" aria-live="polite">
          {shown.length === FEATURES.length ? `All ${FEATURES.length} rows.` : `${shown.length} of ${FEATURES.length} rows.`}
        </p>

        {areas.map(({ area, rows }) => {
          const here = shown.filter((feature) => feature.area === area);
          const tally = { yes: 0, different: 0, no: 0 };
          for (const row of rows) tally[row.answer as Answer] += 1;
          if (!here.length) return null;
          return (
            <section className="mx-area" id={slug(area)} key={area}>
              <header>
                <h3>{area}</h3>
                <div
                  className="mx-meter"
                  role="img"
                  aria-label={`${tally.yes} yes, ${tally.different} another way, ${tally.no} no`}
                >
                  {(["yes", "different", "no"] as const).map(
                    (answer) => tally[answer] > 0 && <i key={answer} className={answer} style={{ flexGrow: tally[answer] }} />,
                  )}
                </div>
                <p className="mx-tally">
                  {tally.yes} of {rows.length}
                </p>
              </header>
              {here.map((feature) => (
                <details className={`mx-row ${feature.answer}`} key={feature.feature}>
                  <summary>
                    <span className="mx-mark" aria-hidden="true">
                      {MARK[feature.answer]}
                    </span>
                    <span className="mx-name">
                      {feature.feature}
                      <span className="sr-only">: {WORD[feature.answer]}</span>
                    </span>
                    <code className="mx-peek">{(feature.oql || feature.refuses || feature.sql).split("\n")[0]}</code>
                  </summary>
                  <div className="mx-tongues">
                    <section>
                      <h4>SQL</h4>
                      <Spoken text={feature.sql} block />
                    </section>
                    <section>
                      <h4>MongoDB</h4>
                      <Spoken text={feature.mongo} block />
                    </section>
                    <section className="owl-side">
                      <h4>Owl</h4>
                      <OwlSide feature={feature} />
                    </section>
                  </div>
                </details>
              ))}
            </section>
          );
        })}
        {!shown.length && <p className="mx-none">Nothing in the matrix matches that.</p>}
      </div>
    </section>
  );
}

function MatrixHud() {
  return (
    <header className="hud">
      <span className="hud-keys">
        <a className="key" href="/">
          <kbd>H</kbd> home
        </a>
        <a className="key" href="/try">
          <kbd>T</kbd> try online
        </a>
      </span>
      <a className="brand" href="/" aria-label="Owl, back to the home page">
        <span className="o-eye">
          <b />
        </span>
        wl
      </a>
      <a className="key" href={GITHUB_URL} target="_blank" rel="noopener noreferrer">
        <kbd>G</kbd> github
      </a>
    </header>
  );
}

export function Matrix() {
  const [picked, setPicked] = useState(0);
  // The tour moves from window to window until the reader picks one.
  const [touring, setTouring] = useState(true);
  const [lit, setLit] = useState(false);
  const street = useRef<HTMLElement>(null);

  useEffect(() => {
    const el = street.current;
    if (!el) return;
    const seen = new IntersectionObserver(
      ([entry]) => {
        if (!entry.isIntersecting) return;
        setLit(true);
        seen.disconnect();
      },
      { threshold: 0.25 },
    );
    seen.observe(el);
    return () => seen.disconnect();
  }, []);

  useEffect(() => {
    if (!touring || !lit || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const timer = setInterval(() => setPicked((at) => (at + STRIDE) % FEATURES.length), 3600);
    return () => clearInterval(timer);
  }, [touring, lit]);

  // The keys shown at the top of the page.
  useEffect(() => {
    const keys = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || /input|textarea/i.test((event.target as HTMLElement).tagName)) return;
      const key = event.key.toLowerCase();
      if (key === "h") location.assign("/");
      if (key === "t") location.assign("/try");
      if (key === "g") window.open(GITHUB_URL, "_blank", "noopener,noreferrer");
    };
    addEventListener("keydown", keys);
    return () => removeEventListener("keydown", keys);
  }, []);

  const pick = (index: number) => {
    setTouring(false);
    setPicked(index);
  };

  return (
    <>
      <InkFilters />
      <MatrixHud />
      <main id="top">
        <section className="hero mx-hero">
          <h1 className="cut">
            <span className="cut-line l1">Can Owl</span>
            <span className="cut-line l2">say that?</span>
          </h1>
          <p className="lede">
            {FEATURES.length} things SQL and MongoDB can say, each one put to Owl.
            <br className="wide" /> Every yes is a query that a test runs and checks. Every no is written down.
          </p>
          <div className="mx-score">
            <a className="mx-tile yes" href="#list">
              <b>
                <CountUp to={COUNT.yes} />
              </b>
              yes
            </a>
            <a className="mx-tile different" href="#list">
              <b>
                <CountUp to={COUNT.different} />
              </b>
              another way
            </a>
            <a className="mx-tile no" href="#list">
              <b>
                <CountUp to={COUNT.no} />
              </b>
              no
            </a>
          </div>
        </section>

        <section className={`night mx-street${lit ? " lit" : ""}`} id="street" ref={street}>
          <div className="wrap">
            <p className="eyebrow">The night shift</p>
            <h2>
              One window
              <br />
              for every question.
            </h2>
            <p className="sub">
              Each building is a part of the language and each window is one thing you might ask of a database. A light
              is on where Owl can do it. Touch a window to read it in all three languages.
            </p>
            <ul className="mx-legend">
              <li>
                <i className="mx-window yes" /> yes
              </li>
              <li>
                <i className="mx-window different" /> another way, on purpose
              </li>
              <li>
                <i className="mx-window no">
                  <i />
                </i>{" "}
                no
              </li>
            </ul>
            <Skyline picked={picked} pick={pick} />
            <Reader feature={FEATURES[picked]} />
            <p className="mx-tour">
              {touring ? (
                <button type="button" onClick={() => setTouring(false)}>
                  Pause the tour
                </button>
              ) : (
                <button type="button" onClick={() => setTouring(true)}>
                  Walk me round
                </button>
              )}
            </p>
          </div>
        </section>

        <Ledger />

        <section className="band mx-proof" id="proof">
          <div className="wrap two">
            <div>
              <p className="eyebrow">How it is checked</p>
              <h2>
                Not a claim.
                <br />A test.
              </h2>
              <p className="sub">
                This page is drawn from the same rows the test runs. Each yes is run against a small shop of five
                customers, five products and six orders, and has to give the answer shown. Each no with something to try
                has to be refused, so a row that starts working fails the test until it is moved.
              </p>
              <p className="sub">
                Only the Owl column is run. The SQL and MongoDB beside it are written by hand to ask the same question
                of the same shop, and are there to compare by eye.
              </p>
              <a
                className="btn solid"
                href={`${GITHUB_URL}/blob/main/docs/COMPLETENESS_MATRIX.md`}
                target="_blank"
                rel="noopener noreferrer"
              >
                Read the matrix on GitHub
              </a>
            </div>
            <div className="slip">
              <span className="stamp">Run it yourself</span>
              <ul>
                <li>
                  The rows: <code>tests/completeness_matrix/rows.rs</code>
                </li>
                <li>
                  The test: <code>tests/completeness_matrix_test.rs</code>
                </li>
                <li>
                  The SQL and MongoDB: <code>tests/completeness_matrix/elsewhere.rs</code>
                </li>
                <li>
                  SQL here means PostgreSQL, with an order's items in a table of their own. MongoDB means mongosh over
                  the same documents as Owl.
                </li>
              </ul>
              <p>
                <code>cargo test --release --test completeness_matrix_test</code>
              </p>
            </div>
          </div>
        </section>
      </main>
      <Footer home="/" />
    </>
  );
}
