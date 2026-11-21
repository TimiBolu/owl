import type { Collection } from "../lib/owl";
import { questionsFor } from "../lib/questions";

/**
 * What is in the database: its collections, and for the one picked, its fields, what it points
 * at, what points at it, and questions worth asking of it. Everything here can be clicked into
 * the question being written.
 */
export function PlayData({
  collections,
  picked,
  onPick,
  onInsert,
  onAsk,
  busy,
}: {
  collections: Collection[];
  picked: string | null;
  onPick: (name: string) => void;
  onInsert: (text: string) => void;
  onAsk: (code: string) => void;
  busy: boolean;
}) {
  const collection = collections.find((c) => c.name === picked) ?? collections[0];
  if (!collection) {
    return (
      <div className="tour-visual data-card">
        <p className="wtable-name">In the database</p>
        <p className="data-empty">Nothing yet. Declare a collection with <code>schema</code>, or bring your own rows below.</p>
      </div>
    );
  }
  return (
    <div className="tour-visual data-card">
      <p className="wtable-name">In the database</p>
      <div className="data-tabs" role="tablist" aria-label="Collections">
        {collections.map((c) => (
          <button key={c.name} role="tab" aria-selected={c.name === collection.name} onClick={() => onPick(c.name)}>
            {c.name}
            <b>{c.rows ?? "?"}</b>
          </button>
        ))}
        <a className="data-own" href="#yours">
          + yours
        </a>
      </div>

      <p className="data-label">
        Fields of {collection.name} <span>click one to write it</span>
      </p>
      <ul className="data-fields">
        {collection.fields.map((field) => (
          <li key={field.name}>
            <button className="data-field" onClick={() => onInsert(`.${field.name}`)} title={`Write .${field.name}`}>
              {field.name}
            </button>
            {field.ref ? (
              <button className="data-type ref" onClick={() => onPick(field.ref!)} title={`Look at ${field.ref}`}>
                {field.type} <i>→</i>
              </button>
            ) : (
              <span className="data-type">{field.type}</span>
            )}
          </li>
        ))}
      </ul>

      {collection.reachedBy.length > 0 && (
        <>
          <p className="data-label">
            Reached from <span>the same references, walked back</span>
          </p>
          <ul className="data-fields">
            {collection.reachedBy.map((reach) => (
              <li key={`${reach.from}.${reach.field}`}>
                <button className="data-field" onClick={() => onInsert(`.${reach.name}`)} title={`Write .${reach.name}`}>
                  {reach.name}
                </button>
                <button className="data-type ref" onClick={() => onPick(reach.from)} title={`Look at ${reach.from}`}>
                  {reach.from}.{reach.field} <i>→</i>
                </button>
              </li>
            ))}
          </ul>
        </>
      )}

      {collection.rules.length > 0 && (
        <>
          <p className="data-label">Rules</p>
          <ul className="data-rules">
            {collection.rules.map((rule) => (
              <li key={rule}>{rule}</li>
            ))}
          </ul>
        </>
      )}

      <p className="data-label">
        Ask about {collection.name} <span>each one runs</span>
      </p>
      <div className="data-asks">
        {questionsFor(collection, collections).map((question) => (
          <button key={question.label} onClick={() => onAsk(question.code)} disabled={busy}>
            {question.label}
          </button>
        ))}
      </div>
    </div>
  );
}
