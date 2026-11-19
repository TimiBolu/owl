import { Tour } from "./Tour";

export function Language() {
  return (
    <section className="band" id="language">
      <div className="wrap">
        <p className="eyebrow">The language · OQL</p>
        <h2>
          Learn the verbs once.
          <br />
          Use them on everything.
        </h2>
        <p className="sub">
          Seven short chapters, from describing your data to questions that are hard in SQL. Every one runs on the
          same six orders, and every answer shown came from a real Owl server.
        </p>
        <Tour />
      </div>
    </section>
  );
}
