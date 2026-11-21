// Reading rows a visitor pastes, as JSON or CSV, and working out a schema
// for them. Nothing here talks to the engine: it reads, guesses and writes
// OQL, and the engine is the judge of whether the rows fit the schema.

export type Kind =
  | { is: "Unknown" }
  | { is: "String" | "Integer" | "Float" | "Boolean" | "DateTime" }
  | { is: "List"; of: Kind }
  | { is: "Object"; fields: Record<string, Guess> };

export type Guess = { kind: Kind; optional: boolean; ref?: string };

export type RowsRead = {
  rows: Array<Record<string, unknown>>;
  /** What was changed or left out on the way, in plain words. */
  notes: string[];
};

const DATE_TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})$/;
const NAME = /^[A-Za-z][A-Za-z0-9_]*$/;

/** A name OQL will take for a field or a collection. */
export function named(raw: string): string {
  const cleaned = raw.trim().replace(/[^A-Za-z0-9_]+/g, "_").replace(/^_+|_+$/g, "");
  if (!cleaned) return "field";
  return /^[A-Za-z]/.test(cleaned) ? cleaned : `f_${cleaned}`;
}

// ---------- Reading ----------

function csv(text: string): string[][] {
  const table: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') (cell += '"'), i++;
      else if (c === '"') quoted = false;
      else cell += c;
    } else if (c === '"' && cell === "") quoted = true;
    else if (c === ",") (row.push(cell), (cell = ""));
    else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(cell);
      cell = "";
      if (row.some((value) => value !== "")) table.push(row);
      row = [];
    } else cell += c;
  }
  row.push(cell);
  if (row.some((value) => value !== "")) table.push(row);
  return table;
}

function cellValue(raw: string): unknown {
  const value = raw.trim();
  if (value === "" || value.toLowerCase() === "null") return null;
  if (value === "true") return true;
  if (value === "false") return false;
  if (/^-?\d+$/.test(value) && Math.abs(Number(value)) <= Number.MAX_SAFE_INTEGER && !/^-?0\d/.test(value)) return Number(value);
  if (/^-?\d+\.\d+$/.test(value)) return Number(value);
  return value;
}

/** Rows from what was pasted: a JSON array of objects, objects one to a line, or CSV with a header. */
export function readRows(pasted: string): RowsRead {
  const text = pasted.trim();
  const notes: string[] = [];
  if (!text) throw new Error("There is nothing to read yet. Paste some rows.");
  let found: unknown[] | null = null;

  if (text.startsWith("[") || text.startsWith("{")) {
    try {
      const parsed: unknown = JSON.parse(text);
      if (Array.isArray(parsed)) found = parsed;
      else if (parsed && typeof parsed === "object") {
        // { "rows": [...] }: the one list inside is the rows.
        const lists = Object.entries(parsed).filter(([, value]) => Array.isArray(value));
        if (lists.length === 1) {
          found = lists[0][1] as unknown[];
          notes.push(`Took the rows from "${lists[0][0]}".`);
        } else found = [parsed];
      }
    } catch (error) {
      // Objects one to a line.
      const lines = text.split(/\r?\n/).filter((line) => line.trim());
      try {
        found = lines.map((line) => JSON.parse(line));
      } catch {
        throw new Error(`That looks like JSON, but it does not read as JSON: ${(error as Error).message}`);
      }
    }
  } else {
    const table = csv(text);
    if (table.length < 2) throw new Error("CSV needs a first line of field names and at least one row under it.");
    const header = table[0].map((name) => name.trim());
    found = table.slice(1).map((cells) => Object.fromEntries(header.map((name, i) => [name, cellValue(cells[i] ?? "")])));
  }

  const rows: Array<Record<string, unknown>> = [];
  let skipped = 0;
  for (const row of found ?? []) {
    if (row && typeof row === "object" && !Array.isArray(row)) rows.push(row as Record<string, unknown>);
    else skipped++;
  }
  if (skipped) notes.push(`${skipped} ${skipped === 1 ? "entry was" : "entries were"} not a row with fields, and left out.`);
  if (!rows.length) throw new Error("No rows with fields were found in that.");

  // Names OQL will take, the same for every row.
  const renamed = new Map<string, string>();
  const rename = (row: Record<string, unknown>): Record<string, unknown> => {
    const out: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(row)) {
      let name = renamed.get(key);
      if (!name) {
        name = NAME.test(key) ? key : named(key);
        renamed.set(key, name);
        if (name !== key) notes.push(`"${key}" is written ${name}.`);
      }
      out[name] = tidy(value);
    }
    return out;
  };
  const tidy = (value: unknown): unknown =>
    Array.isArray(value) ? value.map(tidy) : value && typeof value === "object" ? rename(value as Record<string, unknown>) : value;
  const tidied = rows.map(rename);

  // Every row needs an id, and an id is text.
  if (tidied.every((row) => row.id === undefined || row.id === null)) {
    tidied.forEach((row, i) => (row.id = String(i + 1)));
    notes.push("No id was found, so the rows are numbered: 1, 2, 3 and on.");
  } else {
    const missing = tidied.filter((row) => row.id === undefined || row.id === null).length;
    if (missing) throw new Error(`${missing} ${missing === 1 ? "row has" : "rows have"} no id. Every row needs one.`);
    if (tidied.some((row) => typeof row.id !== "string")) {
      tidied.forEach((row) => (row.id = String(row.id)));
      notes.push("Ids are kept as text.");
    }
    const seen = new Set<unknown>();
    const twice = tidied.find((row) => seen.size === seen.add(row.id).size);
    if (twice) throw new Error(`The id "${twice.id}" is used by more than one row. An id names one row.`);
  }
  return { rows: tidied, notes };
}

// ---------- Guessing ----------

function kindOf(value: unknown): Kind {
  if (value === null || value === undefined) return { is: "Unknown" };
  if (typeof value === "boolean") return { is: "Boolean" };
  if (typeof value === "number") return { is: Number.isInteger(value) ? "Integer" : "Float" };
  if (typeof value === "string") return { is: DATE_TIME.test(value) ? "DateTime" : "String" };
  if (Array.isArray(value)) return { is: "List", of: value.map(kindOf).reduce(both, { is: "Unknown" } as Kind) };
  return { is: "Object", fields: shape([value as Record<string, unknown>]) };
}

/** The one kind two values share, or text when they share none. */
function both(a: Kind, b: Kind): Kind {
  if (a.is === "Unknown") return b;
  if (b.is === "Unknown") return a;
  if (a.is === "List" && b.is === "List") return { is: "List", of: both(a.of, b.of) };
  if (a.is === "Object" && b.is === "Object") {
    const fields: Record<string, Guess> = {};
    for (const name of new Set([...Object.keys(a.fields), ...Object.keys(b.fields)])) {
      const [mine, theirs] = [a.fields[name], b.fields[name]];
      fields[name] =
        mine && theirs
          ? { kind: both(mine.kind, theirs.kind), optional: mine.optional || theirs.optional }
          : { kind: (mine ?? theirs).kind, optional: true };
    }
    return { is: "Object", fields };
  }
  if (a.is === b.is) return a;
  const numbers = new Set(["Integer", "Float"]);
  if (numbers.has(a.is) && numbers.has(b.is)) return { is: "Float" };
  return { is: "String" };
}

function shape(rows: Array<Record<string, unknown>>): Record<string, Guess> {
  const fields: Record<string, Guess> = {};
  const names = new Set(rows.flatMap((row) => Object.keys(row)));
  for (const name of names) {
    let kind: Kind = { is: "Unknown" };
    let optional = false;
    for (const row of rows) {
      const value = row[name];
      if (value === null || value === undefined) optional = true;
      else kind = both(kind, kindOf(value));
    }
    // A field with no value anywhere is taken for text.
    fields[name] = { kind: kind.is === "Unknown" ? { is: "String" } : kind, optional };
  }
  return fields;
}

/**
 * A schema's fields for these rows. `ids` are the ids of the collections already in the database:
 * a field of text whose every value is the id of a row in one of them is taken for a reference.
 */
export function guess(rows: Array<Record<string, unknown>>, ids: Record<string, Set<string>> = {}): Record<string, Guess> {
  const fields = shape(rows);
  for (const [name, field] of Object.entries(fields)) {
    if (name === "id" || field.kind.is !== "String") continue;
    const values = rows.map((row) => row[name]).filter((value): value is string => typeof value === "string");
    if (!values.length) continue;
    const targets = Object.keys(ids).filter((collection) => ids[collection].size && values.every((value) => ids[collection].has(value)));
    // Of several that fit, the one the field is named after.
    const stem = name.toLowerCase().replace(/_?id$/, "");
    const target = targets.find((collection) => collection.toLowerCase().startsWith(stem)) ?? targets[0];
    if (target) field.ref = target;
  }
  return fields;
}

// ---------- Writing ----------

function kindText(kind: Kind): string {
  if (kind.is === "List") return `List(${kindText(kind.of)})`;
  if (kind.is === "Object") {
    const fields = Object.entries(kind.fields).map(([name, field]) => `${name}: ${typeText(field)}`);
    return `{ ${fields.join(", ")} }`;
  }
  // Nothing to go by, as in a list that is always empty: text.
  return kind.is === "Unknown" ? "String" : kind.is;
}

function typeText(guess: Guess): string {
  const written = guess.ref ? `Ref(${guess.ref})` : kindText(guess.kind);
  return guess.optional ? `Optional(${written})` : written;
}

/** The schema as OQL, the id first. */
export function schemaText(collection: string, fields: Record<string, Guess>): string {
  const names = Object.keys(fields).sort((a, b) => Number(b === "id") - Number(a === "id"));
  return `schema ${collection} {\n${names.map((name) => `  ${name}: ${typeText(fields[name])}`).join(",\n")}\n}`;
}

function literal(value: unknown, kind: Kind): string {
  if (value === null || value === undefined) return "null";
  if (kind.is === "List") return `[${(Array.isArray(value) ? value : [value]).map((item) => literal(item, kind.of)).join(", ")}]`;
  if (kind.is === "Object" && typeof value === "object" && !Array.isArray(value)) return object(value as Record<string, unknown>, kind.fields);
  if (kind.is === "Float" && typeof value === "number") return Number.isInteger(value) ? `${value}.0` : String(value);
  if ((kind.is === "Integer" || kind.is === "Boolean") && typeof value !== "string") return String(value);
  // Text, a date and time, or anything of mixed kinds kept as text.
  return JSON.stringify(typeof value === "string" ? value : typeof value === "object" ? JSON.stringify(value) : String(value));
}

function object(row: Record<string, unknown>, fields: Record<string, Guess>): string {
  const written = Object.keys(fields)
    .filter((name) => row[name] !== null && row[name] !== undefined)
    .map((name) => `${name}: ${literal(row[name], fields[name].kind)}`);
  return `{ ${written.join(", ")} }`;
}

/** Inserts for the rows, a few hundred to a statement, so that no one statement is enormous. */
export function inserts(collection: string, rows: Array<Record<string, unknown>>, fields: Record<string, Guess>, each = 200): string[] {
  const statements: string[] = [];
  for (let from = 0; from < rows.length; from += each) {
    const batch = rows.slice(from, from + each).map((row) => `  ${object(row, fields)}`);
    statements.push(`${collection}.insert [\n${batch.join(",\n")}\n]`);
  }
  return statements;
}
