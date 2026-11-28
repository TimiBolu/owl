import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent } from "react";
import { Engine } from "../lib/owl";
import type { About, Answer } from "../lib/owl";
import { SAMPLES, SHOP, SHOP_VECTORS } from "../lib/playShop";
import { GITHUB_URL } from "../lib/seo";
import { usePerch } from "../lib/usePerch";
import { Owl } from "./Hero";
import { PlayData } from "./PlayData";
import { PlayLoad } from "./PlayLoad";
import { Footer, InkFilters } from "./Sections";

/** Rows drawn at most; the rest are counted. */
const SHOWN = 40;
/** Questions remembered at most, in this browser. */
const REMEMBERED = 30;
const ASKED_KEY = "owl-try-asked";
const SHOP_NAMES = ["customers", "orders", "products"];

type Asked = { code: string; rows: number; ms: number; refused: boolean };

const size = (bytes: number) =>
  bytes < 1024 ? `${bytes} bytes` : bytes < 1048576 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / 1048576).toFixed(1)} MB`;

const took = (ms: number) => (ms < 10 ? `${ms.toFixed(1)} ms` : `${Math.round(ms)} ms`);

function shown(value: unknown): string {
  if (value === null || value === undefined) return "null";
  if (typeof value === "object") return JSON.stringify(value).replace(/"([^"]+)":/g, "$1: ").replace(/,/g, ", ");
  return String(value);
}

function remembered(): Asked[] {
  try {
    const kept = JSON.parse(localStorage.getItem(ASKED_KEY) ?? "[]");
    return Array.isArray(kept) ? kept.slice(0, REMEMBERED) : [];
  } catch {
    return [];
  }
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
        <p>{answer.ran === 1 ? "One statement ran" : `${answer.ran} statements ran`}, and the last gave no rows.</p>
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
  const box = useRef<HTMLTextAreaElement>(null);
  const [ready, setReady] = useState<Engine | null>(null);
  const [source, setSource] = useState(SAMPLES[0].code);
  const [tour, setTour] = useState<number | null>(0);
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [about, setAbout] = useState<About | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [asked, setAsked] = useState<Asked[]>([]);
  const [busy, setBusy] = useState(true);
  const [failed, setFailed] = useState<string | null>(null);
  const [run, setRun] = useState(0);
  const [found, setFound] = useState<"new" | "kept" | null>(null);
  // Keep the last words in the bubble while it shrinks away.
  const last = useRef("");
  if (line) last.current = line;

  const collections = about?.collections ?? [];
  // The tour's questions are about the shop, so they are offered while the shop is there.
  const shop = SHOP_NAMES.every((name) => collections.some((c) => c.name === name));

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
      setAsked((before) => {
        const next = [
          { code, rows: answer.rows.length, ms: answer.ms, refused: answer.errors.length > 0 },
          ...before.filter((one) => one.code !== code),
        ].slice(0, REMEMBERED);
        try {
          localStorage.setItem(ASKED_KEY, JSON.stringify(next));
        } catch {}
        return next;
      });
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

  // A database found here already has its data; a new one is given the shop.
  const begin = useCallback(async () => {
    const owl = engine.current!;
    const there = await owl.about();
    if (there.collections.length === 0) {
      const seeded = await owl.run(SHOP);
      if (seeded.answer.errors.length) throw new Error(seeded.answer.errors.join(" "));
      setFound("new");
      return "new";
    }
    // A shop kept from before its products held a vector is given one, so that every
    // sample still has something to answer from. A products collection of the visitor's
    // own shape is left as it is.
    const products = there.collections.find((c) => c.name === "products");
    const isShop = SHOP_NAMES.every((name) => there.collections.some((c) => c.name === name));
    if (isShop && products && !products.fields.some((field) => field.name === "about")) {
      await owl.run(SHOP_VECTORS);
    }
    setFound("kept");
    return "kept";
  }, []);

  useEffect(() => {
    let alive = true;
    const owl = new Engine();
    engine.current = owl;
    owl.onFailure = (message) => alive && setFailed(message);
    setAsked(remembered());
    begin()
      .then(async (which) => {
        if (!alive) return;
        const there = await owl.about();
        const hasShop = SHOP_NAMES.every((name) => there.collections.some((c) => c.name === name));
        const first = hasShop ? SAMPLES[0].code : there.collections[0] ? `${there.collections[0].name}.limit(10)` : "";
        setPicked(hasShop ? "orders" : (there.collections[0]?.name ?? null));
        if (!hasShop) {
          setTour(null);
          setSource(first);
        }
        setReady(owl);
        if (first) await ask(first, true);
        else {
          setAbout(there);
          setBusy(false);
        }
        say(which === "kept" ? "still here." : "ask me.");
      })
      .catch((error) => alive && setFailed(String(error?.message ?? error)));
    return () => {
      alive = false;
      owl.close();
    };
  }, [ask, begin, say]);

  const take = (index: number) => {
    setTour(index);
    setSource(SAMPLES[index].code);
    ask(SAMPLES[index].code);
  };

  /** Put a question in the editor and run it. */
  const pose = (code: string) => {
    setTour(null);
    setSource(code);
    ask(code);
  };

  /** Write something into the question where the cursor is. */
  const insert = (text: string) => {
    const area = box.current;
    const from = area?.selectionStart ?? source.length;
    const to = area?.selectionEnd ?? source.length;
    setSource(`${source.slice(0, from)}${text}${source.slice(to)}`);
    setTour(null);
    requestAnimationFrame(() => {
      area?.focus();
      area?.setSelectionRange(from + text.length, from + text.length);
    });
  };

  const keys = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      if (!busy) ask(source);
    }
    // A tab is two spaces here, as in the samples, not a way out of the box.
    if (event.key === "Tab" && !event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      insert("  ");
    }
  };

  const forget = async () => {
    if (!engine.current) return;
    setBusy(true);
    setAbout(await engine.current.forget());
    await begin();
    setTour(0);
    setPicked("orders");
    setSource(SAMPLES[0].code);
    setAsked([]);
    try {
      localStorage.removeItem(ASKED_KEY);
    } catch {}
    await ask(SAMPLES[0].code, true);
    say("all new.");
  };

  const loaded = async (collection: string) => {
    setPicked(collection);
    setTour(null);
    const code = `${collection}.limit(10)`;
    setSource(code);
    await ask(code, true);
    hop();
    say("got it.");
  };

  const title = tour !== null && shop ? SAMPLES[tour].says : "Your own question.";

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
            <br className="wide" /> See what is in it, ask it anything, or bring your own data.
          </p>
        </section>

        <section className="band play-band" id="ask">
          <div className="wrap">
            {shop && (
              <div className="tour-rail" role="tablist" aria-label="The tour's questions">
                {SAMPLES.map((sample, i) => (
                  <button key={sample.name} role="tab" aria-selected={tour === i} onClick={() => take(i)} disabled={busy}>
                    <b>{String(i + 1).padStart(2, "0")}</b>
                    {sample.name}
                  </button>
                ))}
              </div>
            )}
            <p className="tour-title">{title}</p>

            <div className="bench">
              <aside className="bench-data">
                <PlayData collections={collections} picked={picked} onPick={setPicked} onInsert={insert} onAsk={pose} busy={busy} />
              </aside>

              <div className="bench-ask">
                <div className="code-card form-code play-code">
                  <span className="form-tag">OQL</span>
                  <textarea
                    ref={box}
                    value={source}
                    onChange={(event) => {
                      setSource(event.target.value);
                      setTour(null);
                    }}
                    onKeyDown={keys}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    aria-label="OQL to run"
                    rows={Math.max(6, source.split("\n").length + 1)}
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

                <div className="bench-answer" id="answer">
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
                </div>

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
                      Start over with the shop
                    </button>
                  </p>
                )}

                {asked.length > 0 && (
                  <details className="play-asked">
                    <summary>Asked before · {asked.length}</summary>
                    <ol>
                      {asked.map((one) => (
                        <li key={one.code}>
                          <button onClick={() => pose(one.code)} disabled={busy} title={one.code}>
                            {one.code.replace(/\s+/g, " ")}
                          </button>
                          <span className={one.refused ? "no" : undefined}>
                            {one.refused ? "refused" : `${one.rows} ${one.rows === 1 ? "row" : "rows"}`}
                          </span>
                        </li>
                      ))}
                    </ol>
                  </details>
                )}
              </div>
            </div>
          </div>
        </section>

        <PlayLoad
          engine={ready}
          collections={collections}
          onLoaded={loaded}
          onWrite={(code) => {
            setTour(null);
            setSource(code);
            document.getElementById("ask")?.scrollIntoView({ behavior: "smooth" });
            requestAnimationFrame(() => box.current?.focus());
          }}
        />
      </main>
      <Footer home="/" />
    </>
  );
}
