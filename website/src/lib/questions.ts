// Questions worth asking of a collection, worked out from its schema: so that
// any data, the shop's or a visitor's own, has somewhere to start.
import type { Collection, Field } from "./owl";

export type Question = { label: string; code: string };

const NAMES = ["name", "title", "label", "email", "username", "slug"];
const PILES = ["status", "state", "kind", "type", "category", "country", "city", "level", "plan", "tag", "role"];

const scalar = (field: Field) => !field.list && !field.ref && field.kind !== "Object";
const text = (field: Field) => scalar(field) && field.kind === "String" && field.name !== "id";
const number = (field: Field) => scalar(field) && (field.kind === "Integer" || field.kind === "Float");

/** The field of a collection that best says which row a row is. */
function shownAs(collection: Collection | undefined): string {
  if (!collection) return "id";
  const texts = collection.fields.filter(text);
  return (NAMES.map((name) => texts.find((field) => field.name === name)).find(Boolean) ?? texts[0])?.name ?? "id";
}

export function questionsFor(collection: Collection, all: Collection[]): Question[] {
  const c = collection.name;
  const questions: Question[] = [
    { label: "first rows", code: `${c}.limit(10)` },
    { label: "how many", code: `${c}.count()` },
  ];

  // Through each reference, to the row it points at.
  for (const field of collection.fields.filter((field) => field.ref && !field.list).slice(0, 2)) {
    const shown = shownAs(all.find((other) => other.name === field.ref));
    questions.push({
      label: `with its ${field.name}`,
      code: `${c}\n  .limit(10)\n  .select { id, ${field.name}: .${field.name}.${shown} }`,
    });
  }

  // Back across each reference that points here.
  for (const reach of collection.reachedBy.slice(0, 2)) {
    questions.push({
      label: `most ${reach.name}`,
      code: `${c}\n  .select { id, ${reach.name}: .${reach.name}.count() }\n  .sort { .${reach.name}.desc }\n  .limit(10)`,
    });
  }

  // Into piles by a field that looks like it has few values.
  const texts = collection.fields.filter(text);
  const pile = PILES.map((name) => texts.find((field) => field.name === name)).find(Boolean) ?? texts.find((field) => !NAMES.includes(field.name));
  if (pile) {
    questions.push({
      label: `by ${pile.name}`,
      code: `${c}\n  .group_by { .${pile.name} }\n  .select { ${pile.name}, rows: count() }\n  .sort { .rows.desc }\n  .limit(10)`,
    });
  }

  const measure = collection.fields.find(number);
  if (measure) {
    questions.push({ label: `largest ${measure.name}`, code: `${c}\n  .sort { .${measure.name}.desc }\n  .limit(5)` });
    questions.push({
      label: `add up ${measure.name}`,
      code: `${c}.select {\n  total: sum(.${measure.name}),\n  average: avg(.${measure.name}),\n  largest: max(.${measure.name})\n}`,
    });
  }

  const when = collection.fields.find((field) => scalar(field) && field.kind === "DateTime");
  if (when) questions.push({ label: `latest ${when.name}`, code: `${c}\n  .sort { .${when.name}.desc }\n  .limit(5)` });

  // Inside a list every row holds.
  const list = collection.fields.find((field) => field.list);
  if (list) {
    questions.push(
      /^List\((String|Integer|Float|Boolean)\)/.test(list.type.replace(/^Optional\(/, ""))
        ? {
            label: `count ${list.name}`,
            code: `${c}.${list.name}\n  .group_by { value: . }\n  .select { value, rows: count() }\n  .sort { .rows.desc }\n  .limit(10)`,
          }
        : { label: `inside ${list.name}`, code: `${c}.${list.name}.limit(10)` },
    );
  }
  return questions;
}
