import type { ReactNode } from "react";

/**
 * What a tab holds while another tab is showing. It stays in the page, hidden, so that a
 * search engine reading the HTML, or a person searching it, finds every chapter and not only
 * the one that happens to be open. It is the same content the tab shows when chosen.
 */
export function Offstage({ title, code, notes, answer }: { title: ReactNode; code: string[]; notes?: ReactNode[]; answer?: string[] }) {
  return (
    <div hidden>
      <h3>{title}</h3>
      {code.map((source, i) => (
        <pre key={i}>
          <code dangerouslySetInnerHTML={{ __html: source }} />
        </pre>
      ))}
      {notes && (
        <ul>
          {notes.map((note, i) => (
            <li key={i}>{note}</li>
          ))}
        </ul>
      )}
      {answer && <pre>{answer.join("\n")}</pre>}
    </div>
  );
}
