#!/usr/bin/env python3
"""Run the gauntlet's graph queries on Neo4j, hold each to Owl's answer, and
time it.

The gauntlet checks Owl against SQLite. This asks the other question: on
the queries that are about following relationships, how does Owl do beside
a database built for exactly that?

    cargo build --release -p gauntlet
    python3 apps/gauntlet/neo4j/run.py
    python3 apps/gauntlet/neo4j/run.py 20000      (ten times the rows)
    python3 apps/gauntlet/neo4j/run.py h04 -v     (one query, with its plan)

It needs Docker and the image neo4j:5-community. It starts one container,
loads it with the rows the gauntlet generates, and removes it.

The same rows are given to Neo4j as a graph: a collection is a label, a
reference is a relationship, and the two link collections (assignments,
links) are relationships with properties. See load.cypher. Events and work
logs are left out: none of these queries read them.

What the times are, and are not:

  Owl and SQLite are timed inside the gauntlet's own process. Neo4j is
  asked over HTTP on localhost, in a container, where a request that does
  nothing takes several milliseconds: more than most of these queries.
  So a statement is timed by sending it many times in one request and
  taking away a request that holds it once (see `timed`). Both figures
  are printed: the statement alone, and one request as a client sees it.

  Neo4j Community runs the slotted runtime and the record store format.
  Enterprise has a compiled runtime Neo4j says is about twice as fast.
"""

import csv
import json
import pathlib
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import http.client

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
GAUNTLET = ROOT / "target/release/gauntlet"
NAME, PORT = "owl-gauntlet-neo4j", 17474
PATH = "/db/neo4j/tx/commit"
# One connection kept open: opening one for every statement costs more
# than most of these queries do.
connection = None

# The columns of each file Neo4j loads, in order. Lists and the events are
# left behind.
TABLES = {
    "orgs": ["id", "name", "plan", "region"],
    "units": ["id", "org", "name", "parent", "budget"],
    "people": ["id", "org", "unit", "manager", "name", "level", "salary", "hired", "departed"],
    "projects": ["id", "org", "owner", "name", "state", "budget", "started"],
    "tasks": ["id", "project", "assignee", "parent", "state", "points", "opened", "closed"],
    "assignments": ["id", "person", "project", "role", "hours"],
    "links": ["id", "blocker", "blocked", "kind"],
}


def sh(*args, check=True):
    done = subprocess.run(args, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise RuntimeError(f"{' '.join(map(str, args))}\n{done.stderr.strip()}")
    return done


def cypher(statement, copies=1):
    """Run one statement, or several copies of it in one request. Gives
    (columns, rows, plan or None) of the first."""
    global connection
    body = json.dumps({"statements": [{"statement": statement}] * copies})
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    for attempt in (1, 2):
        try:
            if connection is None:
                connection = http.client.HTTPConnection("127.0.0.1", PORT, timeout=600)
            connection.request("POST", PATH, body, headers)
            answer = json.loads(connection.getresponse().read())
            break
        except (http.client.HTTPException, OSError):
            connection = None
            if attempt == 2:
                raise
    if answer["errors"]:
        raise RuntimeError(answer["errors"][0]["message"].splitlines()[0])
    result = answer["results"][0] if answer["results"] else {"columns": [], "data": []}
    return result["columns"], [line["row"] for line in result["data"]], result.get("plan")


def start(export):
    sh("docker", "rm", "-f", NAME, check=False)
    sh(
        "docker", "run", "-d", "--name", NAME, "-p", f"127.0.0.1:{PORT}:7474",
        "-e", "NEO4J_AUTH=none",
        "-e", "NEO4J_server_memory_heap_initial__size=1G",
        "-e", "NEO4J_server_memory_heap_max__size=1G",
        "-e", "NEO4J_server_memory_pagecache_size=512M",
        "neo4j:5-community",
    )
    with tempfile.TemporaryDirectory() as directory:
        for table, columns in TABLES.items():
            with open(pathlib.Path(directory) / f"{table}.csv", "w", newline="") as out:
                writer = csv.writer(out)
                writer.writerow(columns)
                for line in open(export / f"{table}.jsonl"):
                    row = json.loads(line)
                    writer.writerow(["" if row[column] is None else row[column] for column in columns])
            sh("docker", "cp", f"{directory}/{table}.csv", f"{NAME}:/var/lib/neo4j/import/{table}.csv")
    for _ in range(120):
        try:
            cypher("RETURN 1")
            break
        except Exception:
            time.sleep(1)
    else:
        raise RuntimeError("Neo4j did not come up")
    started = time.perf_counter()
    for statement in (HERE / "load.cypher").read_text().split(";\n"):
        if statement.strip():
            cypher(statement)
    _, counts, _ = cypher("MATCH (n) WITH count(n) AS nodes MATCH ()-[r]->() RETURN nodes, count(r)")
    print(f"Neo4j loaded: {counts[0][0]} nodes, {counts[0][1]} relationships in {time.perf_counter() - started:.1f} s")


def plain(value):
    """A value as it is compared: a whole float is the whole number."""
    if isinstance(value, float):
        value = round(value, 6)
        return int(value) if value == int(value) else value
    if isinstance(value, list):
        return [plain(inner) for inner in value]
    if isinstance(value, dict):
        return {key: plain(inner) for key, inner in value.items()}
    return value


COPIES = 40


def timed(statement, samples):
    """What one run costs Neo4j, in milliseconds, without the round trip.

    A request that holds the statement many times over is timed against a
    request that holds it once. The difference, shared among the extra
    copies, is the statement itself: planning is cached, and the trip to
    the container is in both.
    """

    def request(copies):
        times = []
        for _ in range(samples):
            started = time.perf_counter()
            cypher(statement, copies)
            times.append((time.perf_counter() - started) * 1000)
        return statistics.median(times)

    for _ in range(3):
        cypher(statement)
    once = request(1)
    return max(request(COPIES) - once, 0.0) / (COPIES - 1), once


def hits(plan):
    """Storage hits of a profiled plan, and its operators from the top."""
    total, operators = 0, []

    def walk(node):
        nonlocal total
        total += node.get("DbHits", 0)
        operators.append(node["operatorType"].split("@")[0])
        for child in node.get("children", []):
            walk(child)

    walk(plan["root"])
    return total, operators


def owl_times(samples, people, names):
    """What the gauntlet's own bench prints: name -> (Owl ms, SQLite ms)."""
    out = sh(GAUNTLET, "bench", "--samples", str(samples), "--people", str(people), "--only", names).stdout
    return {
        match[1]: (float(match[2]), float(match[3]))
        for match in re.finditer(r"^(\w+)\s+([\d.]+)\s+([\d.]+)\s+[\d.]+x", out, re.M)
    }


def main():
    samples = 15
    words = [word for word in sys.argv[1:] if not word.startswith("-")]
    people = next((int(word) for word in words if word.isdigit()), 2000)
    only = next((word for word in words if not word.isdigit()), "")
    # The queries that have a Cypher spelling: a second spelling of one
    # (`name_something`) is the same query.
    names = ",".join(sorted({re.match(r"[gh]\d+", path.stem)[0] for path in (HERE / "queries").glob("*.cypher")}))
    export = pathlib.Path(tempfile.mkdtemp(prefix="gauntlet-export-"))
    sh(GAUNTLET, "export", "--to", export, "--people", str(people), "--only", names)
    start(export)
    try:
        nothing, trip = timed("RETURN 1", samples)
        owl = owl_times(samples, people, names)
        print(f"a round trip to Neo4j that does nothing: {trip:.2f} ms; `RETURN 1` itself: {nothing:.3f} ms")
        print("Neo4j ms is the statement alone; 'with trip' is one request as a client sees it.\n")
        print(f"{'query':40}{'Owl ms':>9}{'SQLite ms':>11}{'Neo4j ms':>10}{'with trip':>11}{'db hits':>10}  answer")
        wrong = 0
        for path in sorted((HERE / "queries").glob("*.cypher")):
            name = path.stem
            if only not in name:
                continue
            statement = path.read_text()
            # A second spelling of a query is held to the same answer.
            base = next((known for known in owl if name.startswith(known)), name)
            expected = plain(json.loads((export / "answers" / f"{base}.json").read_text()))
            try:
                columns, rows, _ = cypher(statement)
                found = plain([dict(zip(columns, row)) for row in rows])
                verdict = "same" if found == expected else "DIFFERS"
                if verdict == "DIFFERS":
                    wrong += 1
                    first = next((at for at, pair in enumerate(zip(found, expected)) if pair[0] != pair[1]), min(len(found), len(expected)))
                    verdict += f" at row {first} of {len(found)}/{len(expected)}: {json.dumps(found[first:first + 1])[:160]} | owl {json.dumps(expected[first:first + 1])[:160]}"
                took, once = timed(statement, samples)
                _, _, plan = cypher("PROFILE " + statement)
                total, operators = hits(plan) if plan else (0, [])
                owl_ms, sqlite_ms = owl.get(base, (float("nan"), float("nan")))
                print(f"{name:40}{owl_ms:9.2f}{sqlite_ms:11.2f}{took:10.2f}{once:11.2f}{total:10}  {verdict}")
                if "-v" in sys.argv:
                    print(f"    {' < '.join(operators)}")
            except Exception as error:
                wrong += 1
                print(f"{name:40}  FAILED: {error}")
        sys.exit(1 if wrong else 0)
    finally:
        sh("docker", "rm", "-f", NAME, check=False)
        shutil.rmtree(export, ignore_errors=True)


if __name__ == "__main__":
    main()
