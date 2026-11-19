import type { CSSProperties } from "react";
import { GITHUB_URL, SECTIONS } from "../lib/seo";

const SENTENCE = "Every reference is a path you can walk. No joins. No second language for lists.";

/** Rough-ink filters and the arrowhead shared by every drawing on the page. */
export function InkFilters() {
  return (
    <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
      <filter id="rough" x="-5%" y="-5%" width="110%" height="110%">
        <feTurbulence type="fractalNoise" baseFrequency=".035" numOctaves="2" seed="4" result="n" />
        <feDisplacementMap in="SourceGraphic" in2="n" scale="3.5" />
      </filter>
      <filter id="rough-fine">
        <feTurbulence type="fractalNoise" baseFrequency=".06" numOctaves="2" seed="9" result="n" />
        <feDisplacementMap in="SourceGraphic" in2="n" scale="2.5" />
      </filter>
      <marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
        <path d="M0 0 L10 5 L0 10Z" fill="context-stroke" />
      </marker>
    </svg>
  );
}

export function Hud() {
  return (
    <header className="hud">
      <a className="key" href="#language">
        <kbd>L</kbd> the language
      </a>
      <a className="brand" href="#top" aria-label="Owl, back to top">
        <span className="o-eye look" data-reach="4">
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

/** One sentence that fills in word by word as it is scrolled through. */
export function Sentence() {
  return (
    <section className="say" aria-label={SENTENCE}>
      <p className="say-text" id="say" aria-hidden="true">
        {SENTENCE.split(" ").map((word, i) => (
          <span key={i}>{word} </span>
        ))}
      </p>
    </section>
  );
}

function Eyes({ small, reach, style }: { small?: boolean; reach: number; style: CSSProperties }) {
  return (
    <div className={small ? "night-eyes small" : "night-eyes"} aria-hidden="true" style={style}>
      <i className="look" data-reach={reach}>
        <b />
      </i>
      <i className="look" data-reach={reach}>
        <b />
      </i>
    </div>
  );
}

export function Engine() {
  return (
    <section className="night" id="engine">
      <Eyes reach={6} style={{ top: 84, right: "max(6vw, 24px)" }} />
      <Eyes small reach={3} style={{ top: 210, right: "max(22vw, 150px)", "--wait": "-2s" } as CSSProperties} />
      <Eyes small reach={3} style={{ bottom: 70, left: "max(4vw, 16px)", "--wait": "-4s" } as CSSProperties} />
      <div className="wrap">
        <p className="eyebrow">The engine</p>
        <h2>
          Rust, all
          <br />
          the way down.
        </h2>
        <p className="sub">
          There is no other database underneath. Owl's own engine stores the data, and it knows your schema and your
          references, so it can use both.
        </p>

        <div className="cards">
          <article className="card">
            <span className="num">01</span>
            <h3>References that never break</h3>
            <p>
              Every object has a 64-bit identity that says nothing about where it is stored. Owl can move the data and
              every reference still holds.
            </p>
            <div className="bits" aria-label="Identity layout: 16 bits collection, 32 bits object, 16 bits generation">
              <span style={{ flex: 16 }}>
                collection<small>16</small>
              </span>
              <span style={{ flex: 32 }}>
                object<small>32</small>
              </span>
              <span style={{ flex: 16 }}>
                generation<small>16</small>
              </span>
            </div>
          </article>

          <article className="card">
            <span className="num">02</span>
            <h3>Records shaped by your schema</h3>
            <p>Field names are stored once, not in every record. Ask for two fields and Owl decodes two fields.</p>
            <div className="fig rec" aria-hidden="true">
              <div className="names">
                <b>id</b>
                <b className="on">total</b>
                <b>status</b>
                <b className="on">placed</b>
              </div>
              {[0, 1, 2].map((row) => (
                <div key={row}>
                  <i />
                  <i className="on" />
                  <i />
                  <i className="on" />
                </div>
              ))}
            </div>
          </article>

          <article className="card">
            <span className="num">03</span>
            <h3>Writes that survive a crash</h3>
            <p>
              Every commit is checksummed and synced to disk before any reader sees it. After a crash, Owl loads its
              last checkpoint and replays the rest.
            </p>
            <div className="fig log" aria-hidden="true">
              <span>✓</span>
              <span>✓</span>
              <span>✓</span>
              <span>✓</span>
              <span className="torn">torn</span>
              <small>a half-written entry at the end is dropped</small>
            </div>
          </article>

          <article className="card">
            <span className="num">04</span>
            <h3>Indexes that know the graph</h3>
            <p>
              Every reference gets an index for the way back. You can add ordered, unique, composite and vector
              indexes.
            </p>
            <svg className="fig rev" viewBox="0 0 400 86" aria-hidden="true">
              <path d="M112 30 C170 12 230 12 284 30" markerEnd="url(#arrow)" />
              <path className="back" d="M288 58 C230 76 170 76 116 58" markerEnd="url(#arrow)" />
              <rect x="14" y="22" width="98" height="42" rx="12" />
              <rect x="288" y="22" width="98" height="42" rx="12" />
              <text x="63" y="48">order</text>
              <text x="337" y="48">customer</text>
              <text className="lab" x="200" y="10">
                you declare
              </text>
              <text className="lab" x="200" y="84">
                Owl derives
              </text>
            </svg>
          </article>
        </div>
      </div>
    </section>
  );
}

export function Evidence() {
  return (
    <section className="band" id="evidence">
      <div className="wrap two">
        <div>
          <p className="eyebrow">Evidence</p>
          <h2>
            Right first.
            <br />
            Then fast.
          </h2>
          <p className="sub">
            Two suites give Owl and SQLite the same data, the same indexes and the same durability, then require the
            same rows back. Only then is anything timed. One suite is an ordinary shop. The other is built to be hard.
          </p>
          <div className="ticks" aria-hidden="true">
            {Array.from({ length: 16 }, (_, i) => (
              <i key={i} style={{ "--i": i } as CSSProperties}>
                =
              </i>
            ))}
          </div>
          <p className="ticks-note">
            The shop suite has sixteen queries. Its check fails unless both engines return the same rows for every one.
          </p>
          <a className="btn" href={`${GITHUB_URL}/tree/main/apps`} target="_blank" rel="noopener noreferrer">
            Read the suites
          </a>
        </div>
        <aside className="slip">
          <span className="stamp">Not for production yet</span>
          <h3>What Owl is not, today</h3>
          <ul>
            <li>It runs on one machine. The clustering code is experimental.</li>
            <li>Writes are applied one at a time.</li>
            <li>It has no syntax yet for changing a schema.</li>
          </ul>
          <p>Owl is under active development. Keep data you cannot lose somewhere else for now.</p>
        </aside>
      </div>
    </section>
  );
}

// Each command here was run against a real owl-server; the last line is its actual answer.
const QUICK_START = `<span class="c"># 1. Start a server (needs only a Rust toolchain)</span>
cargo run --release -p owl-server -- --data shop.owl

<span class="c"># 2. Declare two collections and add a row to each</span>
curl 127.0.0.1:7878/v1/script --json '{"source": "schema customers { id: String, name: String }\\nschema orders { id: String, customer: Ref(customers), total: Float }\\ncustomers.insert { id: \\"c1\\", name: \\"Ada\\" }\\norders.insert { id: \\"o1\\", customer: \\"c1\\", total: 12.5 }"}'

<span class="c"># 3. Follow the reference from order to customer</span>
curl 127.0.0.1:7878/v1/query --json '{"query": "orders.select { id, who: .customer.name }"}'

<span class="out">{"rows":[{"id":"o1","who":"Ada"}]}</span>`;

export function Start() {
  return (
    <section className="band start" id="start">
      <div className="wrap">
        <p className="eyebrow">Run it</p>
        <h2>
          Three commands
          <br />
          to a first answer.
        </h2>
        <div className="code-card wide-card">
          <pre>
            <code dangerouslySetInnerHTML={{ __html: QUICK_START }} />
          </pre>
        </div>
        <div className="cta">
          <a className="btn solid" href={GITHUB_URL} target="_blank" rel="noopener noreferrer">
            Get Owl on GitHub
          </a>
        </div>
      </div>
    </section>
  );
}

// What the README lists as the most useful contributions right now.
const WANTED = ["crash tests", "format fuzzing", "the record codec", "key encoding", "index tests", "repeatable benchmarks"];

/** A plain ask, at the end of the page: a star, a try, or a hand. */
export function Join() {
  return (
    <section className="band join" id="join">
      <div className="wrap">
        <p className="eyebrow">Join in</p>
        <h2>
          Owl is young.
          <br />
          Help it grow.
        </h2>
        <p className="sub">
          Owl is open source under the MIT license, and it is early. If you like where it is going, here are three ways
          to help, from ten seconds to a weekend.
        </p>

        <div className="join-cards">
          <article className="join-card star-card">
            <svg className="big-star" viewBox="0 0 120 120" aria-hidden="true">
              <path filter="url(#rough-fine)" d="M60 8 L74 44 L112 46 L82 70 L92 108 L60 86 L28 108 L38 70 L8 46 L46 44Z" />
            </svg>
            <span className="num">Ten seconds</span>
            <h3>Give it a star</h3>
            <p>A star costs nothing, and it is how other people come across a project this new.</p>
            <a className="btn solid" href={GITHUB_URL} target="_blank" rel="noopener noreferrer">
              Star Owl on GitHub
            </a>
          </article>

          <article className="join-card">
            <span className="num">Ten minutes</span>
            <h3>Try it and say what broke</h3>
            <p>Run the three commands above against data shaped like yours. A query Owl gets wrong, or cannot say, is the most useful report there is.</p>
            <a className="btn" href={`${GITHUB_URL}/issues/new`} target="_blank" rel="noopener noreferrer">
              Open an issue
            </a>
          </article>

          <article className="join-card">
            <span className="num">A weekend</span>
            <h3>Help build it</h3>
            <p>The priority right now is getting storage right. Good places to start:</p>
            <p className="wanted">
              {WANTED.map((item) => (
                <i key={item}>{item}</i>
              ))}
            </p>
            <a className="btn" href={`${GITHUB_URL}#help-owl-grow`} target="_blank" rel="noopener noreferrer">
              How to contribute
            </a>
          </article>
        </div>
      </div>
    </section>
  );
}

export function Footer() {
  return (
    <footer className="foot">
      <nav className="foot-nav" aria-label="Sections of this page">
        {SECTIONS.map(([id, name]) => (
          <a key={id} href={`#${id}`}>
            {name}
          </a>
        ))}
        <a href={GITHUB_URL} target="_blank" rel="noopener noreferrer">
          GitHub
        </a>
      </nav>
      <span>owl</span>
      <span>An object-graph database, written in Rust. MIT licensed. Still awake.</span>
    </footer>
  );
}
