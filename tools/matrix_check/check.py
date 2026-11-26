#!/usr/bin/env python3
"""Run the SQL and MongoDB the matrix page shows, on a real PostgreSQL and a
real MongoDB, and say which ran and which gave the rows Owl gave.

The page sets a whole SQL statement and a whole MongoDB statement beside
each OQL query. Owl's are run by the matrix test. These were written by
hand, and this is what checks them: each is run against the same shop, and
where its rows can be compared with the rows the test holds Owl to, they
are.

    python3 tools/matrix_check/check.py

It needs Docker and the images postgres:17 and mongo:7. It starts a
container of each, uses them, and removes them. Nothing else is touched.

A statement comes out one of five ways:

  same       it ran and gave the rows Owl gives
  reordered  it gave those rows in another order
  ran        it ran; there is nothing to compare (a write, a definition)
  shaped     it ran and answers in another shape, for a reason given
  differs    it ran and gave other rows: read the report
  failed     it did not run: read the report

`differs` is not always a mistake. SQL and MongoDB name and shape their
answers their own way, and the comparison only looks at the values in each
row. The report shows both sides so that a person can tell.
"""

import json
import pathlib
import re
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
PG, MONGO = "owl-matrix-pg", "owl-matrix-mongo"

# What cannot be run here, and why.
SKIP_SQL = {
    "Nearest vectors": "needs the pgvector extension",
    "Places and distances": "needs PostGIS",
}
# Statements that run and answer in another shape than Owl's, on purpose
# or by the nature of the other database. They are shown, not counted
# against the page.
SHAPED = {
    ("sql", "Every row with every row"): "one flat row a pair; Owl nests the second side, as the row says",
    ("sql", "First and last of a partition"): "the last row's id; Owl gives the last row as a list of one",
    ("mongo", "First and last of a partition"): "the last row's id; Owl gives the last row as a list of one",
    ("mongo", "Ranges as groups"): "a bucket is named by its lower bound, and comes in that order",
    ("mongo", "Median and percentiles"): "MongoDB picks a value that is there; Owl and SQL go between two",
}

SKIP_MONGO = {
    "Several writes kept or dropped together": "a transaction needs a replica set",
    "Choice of isolation level": "a transaction needs a replica set",
    "One collection split by a key": "needs a sharded cluster",
    "Full-text search": "needs a text index the row does not make",
    "Places and distances": "needs a 2dsphere index the row does not make",
    "Code that runs when rows change": "a change stream needs a replica set",
    "Being told when rows change": "a change stream needs a replica set",
    "Users, roles and permissions": "asks for a password at a prompt",
}


def sh(*args, stdin=None, check=True):
    done = subprocess.run(args, input=stdin, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise RuntimeError(f"{' '.join(args)}\n{done.stderr.strip()}")
    return done


def start():
    for name in (PG, MONGO):
        sh("docker", "rm", "-f", name, check=False)
    sh("docker", "run", "-d", "--name", PG, "-e", "POSTGRES_HOST_AUTH_METHOD=trust", "-e", "TZ=UTC", "postgres:17")
    sh("docker", "run", "-d", "--name", MONGO, "mongo:7")
    for _ in range(60):
        ready = sh("docker", "exec", PG, "pg_isready", "-U", "postgres", check=False).returncode == 0
        if ready and psql("postgres", "SELECT 1", check=False).returncode == 0:
            break
        sh("sleep", "0.5")
    for _ in range(60):
        if sh("docker", "exec", MONGO, "mongosh", "--quiet", "--eval", "1", check=False).returncode == 0:
            break
        sh("sleep", "0.5")
    psql("postgres", "CREATE DATABASE shop; ALTER DATABASE shop SET timezone = 'UTC';")
    psql("shop", (HERE / "shop.sql").read_text())
    psql("postgres", "CREATE DATABASE extras TEMPLATE shop; ALTER DATABASE extras SET timezone = 'UTC';")
    psql("extras", (HERE / "extras.sql").read_text())
    sh("docker", "cp", str(HERE / "shop.js"), f"{MONGO}:/shop.js")


def stop():
    for name in (PG, MONGO):
        sh("docker", "rm", "-f", name, check=False)


def psql(database, text, check=True):
    return sh(
        "docker", "exec", "-i", PG, "psql", "-U", "postgres", "-d", database,
        "-v", "ON_ERROR_STOP=1", "-q", "-At", stdin=text, check=check,
    )


def one_query(sql):
    """The statement, if it is a single query whose rows can be asked for."""
    lines = [line for line in sql.splitlines() if not line.strip().startswith("--")]
    text = "\n".join(lines).strip()
    if not text.endswith(";") or ";" in text[:-1]:
        return None
    if not re.match(r"(?is)^(select|with)\b", text):
        return None
    return text[:-1]


def run_sql(sql):
    """Run a statement in a copy of the shop. Gives (rows or None, error)."""
    query = one_query(sql)
    text = f"SELECT coalesce(json_agg(row_to_json(t)), '[]') FROM ({query}) t;" if query else sql
    error = ""
    for template in ("shop", "extras"):
        psql("postgres", f"DROP DATABASE IF EXISTS scratch; CREATE DATABASE scratch TEMPLATE {template};")
        psql("postgres", "ALTER DATABASE scratch SET timezone = 'UTC';")
        done = psql("scratch", text, check=False)
        if done.returncode == 0:
            return (json.loads(done.stdout.strip() or "[]") if query else None), ""
        error = done.stderr.strip().splitlines()[0] if done.stderr.strip() else "failed"
        # Only a table the row takes as given is a reason to try again.
        if "does not exist" not in error:
            break
    return None, error


RUNNER = """
load("/shop.js");
db = db.getSiblingDB("shop");
let out;
try {
%s
  if (out && typeof out.toArray === "function") out = out.toArray();
  print("@@ok" + EJSON.stringify(out === undefined ? null : out, { relaxed: true }));
} catch (error) {
  print("@@no" + String(error.message || error).split("\\n")[0]);
}
"""


def keeping_the_last(statement):
    """The statement as a script that keeps what its last part gives.

    It is written into the script as it stands, not handed to `eval`:
    mongosh makes its calls wait for their answers only in code it can
    see. The last part is from the last line that begins a statement.
    """
    lines = statement.splitlines()
    starts = [
        at
        for at, line in enumerate(lines)
        if line and line[0] not in " \t).]}" and not line.startswith("//")
    ]
    if not starts:
        return statement
    last = starts[-1]
    keep = "" if re.match(r"(const|let|var)\b", lines[last]) else "out = "
    return "\n".join(lines[:last] + [keep + lines[last]] + lines[last + 1 :])


def run_mongo(statement):
    sh("docker", "exec", "-i", MONGO, "sh", "-c", "cat > /run.js", stdin=RUNNER % keeping_the_last(statement))
    done = sh("docker", "exec", MONGO, "mongosh", "--quiet", "--file", "/run.js", check=False)
    for line in done.stdout.splitlines():
        if line.startswith("@@ok"):
            value = json.loads(line[4:])
            if isinstance(value, list):
                return value, ""
            # One value, or an acknowledgement of a write.
            scalar = not isinstance(value, dict)
            return ([{"value": value}] if scalar else None), ""
        if line.startswith("@@no"):
            return None, line[4:]
    return None, (done.stderr.strip() or done.stdout.strip() or "no answer").splitlines()[-1]


def plain(value):
    """A value as it is compared: whole numbers as whole numbers, a date as
    its day or its instant, a missing field the same as a null one."""
    if isinstance(value, float):
        value = round(value, 9)
        return int(value) if value == int(value) else value
    if isinstance(value, dict):
        if set(value) == {"$date"}:
            return plain(value["$date"])
        return {("id" if key == "_id" else key): plain(inner) for key, inner in value.items() if inner is not None}
    if isinstance(value, list):
        return [plain(inner) for inner in value]
    if isinstance(value, str):
        # A timestamp at midnight UTC is the day.
        day = re.match(r"^(\d{4}-\d{2}-\d{2})T00:00:00(\.0+)?(Z|\+00:00)$", value)
        return day.group(1) if day else value
    return value


def shape_free(value):
    """A value with its field names set aside: SQL, MongoDB and Owl name
    the same things differently, and it is the values that are compared."""
    if isinstance(value, dict):
        return sorted((shape_free(inner) for inner in value.values()), key=json.dumps)
    if isinstance(value, list):
        return [shape_free(inner) for inner in value]
    return value


def values_of(row):
    row = plain(row)
    if not isinstance(row, dict):
        # A bare value is a row of that one value.
        return [json.dumps(shape_free(row))]
    # MongoDB puts the keys of a group together under _id.
    if isinstance(row.get("id"), dict):
        row = {**{key: inner for key, inner in row.items() if key != "id"}, **row["id"]}
    return sorted(json.dumps(shape_free(inner)) for inner in row.values())


def owl_rows(gives):
    rows, decoder, at = [], json.JSONDecoder(), 0
    while at < len(gives):
        if gives[at].isspace():
            at += 1
            continue
        try:
            row, at = decoder.raw_decode(gives, at)
        except json.JSONDecodeError:
            return None
        rows.append(row)
    return rows


def verdict(rows, expected):
    if rows is None or expected is None:
        return "ran"
    got, want = [values_of(row) for row in rows], [values_of(row) for row in expected]
    if got == want:
        return "same"
    if sorted(map(json.dumps, got)) == sorted(map(json.dumps, want)):
        return "reordered"
    return "differs"


def main():
    matrix = json.loads((ROOT / "website/src/lib/matrix.json").read_text())
    only = sys.argv[1] if len(sys.argv) > 1 else None
    start()
    counts = {"sql": {}, "mongo": {}}
    report = []
    try:
        for area in matrix:
            for row in area["rows"]:
                feature = row["feature"]
                if only and only.lower() not in feature.lower():
                    continue
                # A whole statement, not the short label a row without OQL keeps.
                expected = owl_rows(row["gives"]) if row["gives"] and not row["check"] else None
                for side, text, skip, run, whole in (
                    ("sql", row["sql"], SKIP_SQL, run_sql, ";" in row["sql"] and not row["sql"].startswith("(")),
                    ("mongo", row["mongo"], SKIP_MONGO, run_mongo, "db." in row["mongo"] and not row["mongo"].startswith("(")),
                ):
                    if not whole or not (row["oql"] or row["refuses"] or "\n" in text):
                        continue
                    if feature in skip:
                        outcome, detail = "skipped", skip[feature]
                    else:
                        rows, error = run(text)
                        outcome = "failed" if error else verdict(rows, expected)
                        detail = error
                        if outcome in ("differs", "reordered"):
                            detail = f"gave   {json.dumps([plain(r) for r in rows])}\n  owl    {json.dumps([plain(r) for r in expected])}"
                        if outcome == "differs" and (side, feature) in SHAPED:
                            outcome = "shaped"
                            detail = f"{SHAPED[(side, feature)]}\n  {detail}"
                    counts[side][outcome] = counts[side].get(outcome, 0) + 1
                    if outcome in ("failed", "differs", "reordered", "skipped", "shaped"):
                        report.append(f"[{side} {outcome}] {area['area']} / {feature}\n  {detail}\n")
    finally:
        stop()
    print("\n".join(report))
    for side in ("sql", "mongo"):
        total = sum(counts[side].values())
        parts = ", ".join(f"{count} {outcome}" for outcome, count in sorted(counts[side].items()))
        print(f"{side}: {total} statements: {parts}")
    bad = sum(counts[side].get("failed", 0) + counts[side].get("differs", 0) for side in counts)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
