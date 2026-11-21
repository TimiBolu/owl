// The engine, as the page sees it. It runs in a worker (public/owl/worker.js),
// where a browser lets it keep its files; the page sends what to run and is
// sent back what came of it.

export type Row = Record<string, unknown>;

export type Answer = {
  /** The rows of the last statement, when it was a query. */
  rows: Row[];
  /** Their fields, in the order the query selected them. */
  columns: string[];
  /** How many statements ran. */
  ran: number;
  /** What went wrong, in the engine's own words. */
  errors: string[];
  /** How long the engine took, in milliseconds. */
  ms: number;
};

export type Field = {
  name: string;
  /** The type as OQL writes it: `String`, `Ref(customers)`, `List({ product: Ref(products), qty: Integer })`. */
  type: string;
  /** The collection the field refers to, when it is a reference or a list of them. */
  ref: string | null;
  optional: boolean;
  list: boolean;
  /** The type with `Optional` taken off: `String`, `Integer`, `Float`, `Boolean`, `DateTime`, `Reference`, `List`, `Object`. */
  kind: string;
};

export type Collection = {
  name: string;
  rows: number | null;
  /** The id, then the rest by name: the engine does not keep the order they were written in. */
  fields: Field[];
  /** The references of other collections that point here, and the name each is reached by. */
  reachedBy: Array<{ name: string; from: string; field: string }>;
  rules: string[];
};

export type About = {
  /** Whether the database is kept in this browser, or only held in memory. */
  kept: boolean;
  /** Why it is not kept, when it is not. */
  why: string;
  files: number;
  bytes: number;
  /** What is in the database now. */
  collections: Collection[];
};

type Reply = { id: number; result?: Answer; about: About };

export class Engine {
  private worker: Worker;
  private waiting = new Map<number, (reply: Reply) => void>();
  private asked = 0;
  /** Called if the worker itself fails, as when the engine cannot be fetched. */
  onFailure: (message: string) => void = () => {};

  constructor() {
    this.worker = new Worker("/owl/worker.js", { type: "module" });
    this.worker.onmessage = ({ data }: MessageEvent<Reply>) => {
      this.waiting.get(data.id)?.(data);
      this.waiting.delete(data.id);
    };
    this.worker.onerror = (event) => this.onFailure(event.message || "the engine did not start");
  }

  /** What is in the database, without running anything. */
  async about() {
    return (await this.ask("about")).about;
  }

  private ask(kind: "run" | "forget" | "about", source?: string) {
    return new Promise<Reply>((answered) => {
      const id = ++this.asked;
      this.waiting.set(id, answered);
      this.worker.postMessage({ id, kind, source });
    });
  }

  /** Run OQL: schemas, inserts, functions and queries, one after another. */
  async run(source: string) {
    const reply = await this.ask("run", source);
    return { answer: reply.result!, about: reply.about };
  }

  /** Remove the database kept in this browser and begin an empty one. */
  async forget() {
    return (await this.ask("forget")).about;
  }

  close() {
    this.worker.terminate();
  }
}
