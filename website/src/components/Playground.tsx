import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent } from "react";
import { Engine } from "../lib/owl";
import type { About, Answer } from "../lib/owl";
import { SAMPLES, SCHEMA, SHOP } from "../lib/playShop";
import { GITHUB_URL } from "../lib/seo";
import { usePerch } from "../lib/usePerch";
import { Owl } from "./Hero";
import { Footer, InkFilters } from "./Sections";

/** Rows drawn at most; the rest are counted. */
const SHOWN = 40;

const size = (bytes: number) =>
  bytes < 1024 ? `${bytes} bytes` : bytes < 1048576 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / 1048576).toFixed(1)} MB`;

const took = (ms: number) => (ms < 10 ? `${ms.toFixed(1)} ms` : `${Math.round(ms)} ms`);

function shown(value: unknown): string {
  if (value === null || value === undefined) return "null";
  if (typeof value === "object") return JSON.stringify(value).replace(/"([^"]+)":/g, "$1: ").replace(/,/g, ", ");
  return String(value);
}

function PlayHud() {
  return (
    <header className="hud">
      <a className="key" href="/">
        <kbd>H</kbd> home
      </a>
      <a className="brand" href="/" aria-label="Owl, back to the home page">
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

/** What the engine gave back: rows, nothing, or what it refused. */
function AnswerCard({ answer, run }: { answer: Answer | null; run: number }) {
  if (!answer) {
    return (
      <div className="verdict waiting play-verdict">
        <p>The engine is waking up.</p>
      </div>
    );
  }
  if (answer.errors.length) {
    return (
      <div className="verdict refused play-verdict play-shake" key={run}>
        <span className="stamp-mark">Refused</span>
        {answer.errors.map((error, i) => (
          <p key={i}>{error}</p>
        ))}
      </div>
    );
  }
  if (!answer.rows.length) {
    return (
      <div className="verdict done play-verdict" key={run}>
        <span className="stamp-mark">Done</span>
        <p>
          {answer.ran === 1 ? "One statement ran" : `${answer.ran} statements ran`}, and the last gave no rows.
        </p>
      </div>
    );
  }
  const columns = answer.columns;
  const more = answer.rows.length - SHOWN;
  return (
    <div className="tour-visual wtable play-table" key={run}>
      <p className="wtable-name">
        {answer.rows.length === 1 ? "1 row" : `${answer.rows.length} rows`} · {took(answer.ms)}
      </p>
      <div className="play-rows" style={{ "--cols": columns.length } as CSSProperties}>
        <div className="wrow whead">
          {columns.map((name) => (
            <span key={name}>{name}</span>
          ))}
        </div>
        {answer.rows.slice(0, SHOWN).map((row, i) => (
          <div className="wrow prow" key={i} style={{ "--i": Math.min(i, 12) } as CSSProperties}>
            {columns.map((name) => {
              const text = shown(row[name]);
              return (
                <span key={name} title={text} className={row[name] === null || row[name] === undefined ? "nil" : undefined}>
                  {text}
                </span>
              );
            })}
          </div>
        ))}
      </div>
      {more > 0 && <p className="play-more">and {more} more.</p>}
    </div>
  );
}

export function Playground() {
  const { line, say, hop } = usePerch();
  const engine = useRef<Engine | null>(null);
  const [source, setSource] = useState(SAMPLES[0].code);
  const [picked, setPicked] = useState<number | null>(0);
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [about, setAbout] = useState<About | null>(null);
  const [busy, setBusy] = useState(true);
  const [failed, setFailed] = useState<string | null>(null);
  const [run, setRun] = useState(0);
  const [found, setFound] = useState<"new" | "kept" | null>(null);
  // Keep the last words in the bubble while it shrinks away.
  const last = useRef("");
  if (line) last.current = line;

  const ask = useCallback(
    async (code: string, quiet = false) => {
      if (!engine.current) return;
      setBusy(true);
      const { answer, about } = await engine.current.run(code);
      setBusy(false);
      setAnswer(answer);
      setAbout(about);
      setRun((n) => n + 1);
      if (quiet) return;
      // On a phone the answer is below what was asked: bring it into view.
      if (matchMedia("(max-width: 820px)").matches) {
        document.getElementById("answer")?.scrollIntoView({ block: "start", behavior: "smooth" });
      }
      if (answer.errors.length) say("hm. no.");
      else {
        hop();
        say(answer.rows.length ? (answer.rows.length === 1 ? "one row." : `${answer.rows.length} rows.`) : "done.");
      }
    },
    [say, hop],
  );

  // A database found here already has its shop; a new one is given it.
  const begin = useCallback(async () => {
    const owl = engine.current!;
    const empty = (asked: Answer) => asked.errors.length > 0 || asked.rows[0]?.count === 0;
    const orders = await owl.run("orders.count()");
    const customers = await owl.run("customers.count()");
    if (empty(orders.answer) && empty(customers.answer)) {
      const seeded = await owl.run(SHOP);
      if (seeded.answer.errors.length) throw new Error(seeded.answer.errors.join(" "));
      setFound("new");
      return "new";
    }
    setFound("kept");
    return "kept";
  }, []);

  useEffect(() => {
    let alive = true;
    const owl = new Engine();
    engine.current = owl;
    owl.onFailure = (message) => alive && setFailed(message);
    begin()
      .then(async (which) => {
        if (!alive) return;
        await ask(SAMPLES[0].code, true);
        say(which === "kept" ? "still here." : "ask me.");
      })
      .catch((error) => alive && setFailed(String(error?.message ?? error)));
    return () => {
      alive = false;
      owl.close();
    };
  }, [ask, begin, say]);

  const pick = (index: number) => {
    setPicked(index);
    setSource(SAMPLES[index].code);
    ask(SAMPLES[index].code);
  };

  const keys = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      if (!busy) ask(source);
    }
    // A tab is two spaces here, as in the samples, not a way out of the box.
    if (event.key === "Tab" && !event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      const box = event.currentTarget;
      const { selectionStart: from, selectionEnd: to } = box;
      const next = `${source.slice(0, from)}  ${source.slice(to)}`;
      setSource(next);
      setPicked(null);
      requestAnimationFrame(() => box.setSelectionRange(from + 2, from + 2));
    }
  };

  const forget = async () => {
    if (!engine.current) return;
    setBusy(true);
    setAbout(await engine.current.forget());
    await begin();
    setPicked(0);
    setSource(SAMPLES[0].code);
    await ask(SAMPLES[0].code, true);
    say("all new.");
  };

  return (
    <>
      <InkFilters />
      <PlayHud />
      <main id="top">
        <section className="hero play-hero">
          <h1 className="cut">
            <span className="cut-line l1">Ask it</span>
            <span className="cut-line l2">yourself.</span>
          </h1>
          <p className="lede">
            The real engine, running in this tab. No server is answering.
            <br className="wide" /> What you write stays in your browser.
          </p>
        </section>

        <section className="band play-band">
          <div className="wrap">
            <div className="tour-rail" role="tablist" aria-label="Samples to run">
              {SAMPLES.map((sample, i) => (
                <button key={sample.name} role="tab" aria-selected={picked === i} onClick={() => pick(i)} disabled={busy}>
                  <b>{String(i + 1).padStart(2, "0")}</b>
                  {sample.name}
                </button>
              ))}
            </div>
            <p className="tour-title">{picked === null ? "Your own question." : SAMPLES[picked].says}</p>

            <div className="play-stage">
              <div className="play-left">
                <div className="code-card form-code play-code">
                  <span className="form-tag">OQL</span>
                  <textarea
                    value={source}
                    onChange={(event) => {
                      setSource(event.target.value);
                      setPicked(null);
                    }}
                    onKeyDown={keys}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    aria-label="OQL to run"
                    rows={Math.max(8, source.split("\n").length + 1)}
                  />
                  <div className="play-bar">
                    <button className="run strong" onClick={() => ask(source)} disabled={busy || !!failed}>
                      {busy ? "Running" : "Run"}
                    </button>
                    <span className="play-hint">
                      <kbd>⌘</kbd>
                      <kbd>↵</kbd>
                    </span>
                  </div>
                </div>

                <details className="play-schema">
                  <summary>What is in the shop</summary>
                  <pre>{SCHEMA}</pre>
                  <p>
                    Three customers, four products and six orders: the same shop the{" "}
                    <a href="/#language">language tour</a> draws. Add your own collections with <code>schema</code>.
                  </p>
                </details>
              </div>

              <div className="play-right" id="answer">
                <div className="perch play-perch">
                  <Owl hoot={() => say("who?")} />
                  <span className={line ? "bubble on" : "bubble"} role="status" aria-live="polite">
                    {last.current}
                  </span>
                </div>
                {failed ? (
                  <div className="verdict refused play-verdict">
                    <span className="stamp-mark">Asleep</span>
                    <p>The engine did not start in this browser: {failed}</p>
                  </div>
                ) : (
                  <AnswerCard answer={answer} run={run} />
                )}

                {about && (
                  <p className={about.kept ? "play-kept" : "play-kept not"}>
                    {about.kept ? (
                      <>
                        <b>{found === "kept" ? "Found your database." : "Kept in this browser."}</b> {about.files}{" "}
                        {about.files === 1 ? "file" : "files"}, {size(about.bytes)}. Reload the page and it is still here.
                      </>
                    ) : (
                      <>
                        <b>Held in memory only.</b>{" "}
                        {about.why === "another tab"
                          ? "Another tab has this database open, so this one starts fresh and forgets on closing."
                          : "This browser gave no files to keep it in, so it is gone when the tab closes."}
                      </>
                    )}{" "}
                    <button className="play-forget" onClick={forget} disabled={busy}>
                      Start over
                    </button>
                  </p>
                )}
              </div>
            </div>
          </div>
        </section>
      </main>
      <Footer home="/" />
    </>
  );
}
