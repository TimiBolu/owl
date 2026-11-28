# OQL on one page

For a program that writes queries. Every example here is run by the test
suite against the schema below, so each is known to be accepted. The full
language is in `OQL_SPECIFICATION.md`.

## The example database

```oql
schema people { id: String, name: String, level: Integer, manager: Optional(Ref(people)) as reports }
schema projects { id: String, name: String, owner: Ref(people) as owned }
schema assignments { id: String, person: Ref(people), project: Ref(projects), role: String, hours: Integer }
schema tasks { id: String, project: Ref(projects), state: String, tags: List(String), notes: List({ kind: String, mins: Integer }), about: Optional(Vector(3)) }
schema links { id: String, blocker: Ref(tasks) as blocking, blocked: Ref(tasks) as blocked_by, kind: String }
```

`manager: Ref(people) as reports` gives two names: `.manager` on a person
is their manager, and `.reports` on a person is the people they manage. A
reference with no `as` is reached backwards by the referring collection's
name: `.assignments` on a person or a project, `.tasks` on a project.

## The shape of a query

A query starts from a collection and applies operations left to right.
Each operation sees the rows the one before it gave.

```oql
people.where { .level >= 2 && .name != "Kim" }.sort { .level desc, .name }.limit(3).select { name, level }
```

- A field of the row is `.name`. A bare `name` is the same in `select`.
- Equality is `==`. Text is in double quotes. And, or, not: `&&`, `||`, `!`.
- With no `sort`, rows come in order of `id`. A filter never reorders.
- `limit(n)`, `offset(n)`, `first()` take from the order the rows are in.

Conditions:

```oql
people.where { .level between 2 and 4 }

people.where { .name in ["Kim", "Lee"] || .name like "A%" }

people.where { .manager is null }

tasks.where { .tags contains "api" }

people.where { .name.lower().starts_with("a") }
```

## Paths follow references, both ways

```oql
people.where { .manager.name == "Kim" }.select { name, boss: .manager.name }

projects.select { name, owner: .owner.name, people: .assignments.count() }

people.where { .reports.exists() }.select { name, team: .reports.select { name, level } }
```

- A path through a missing reference is `null`, never an error.
- A reference equals the id it points at: `.owner == "kim"`.
- A path through a collection is a list of values: `.assignments.hours`.
  It cannot be compared with one value. Reduce it, or ask whether any row
  passes:

```oql
people.where { .assignments.hours.sum() > 20 }

people.where { .assignments.where { .role == "lead" }.exists() }

people.where { !.assignments.exists() }
```

## Select

```oql
assignments.select { id, who: .person.name, weekly: .hours * 5, heavy: .hours > 15 }

people.select { *, -manager, boss: .manager.name }

people.select { name, band: if .level >= 3 then "senior" else "junior" }

people.select { name, band: case { .level >= 3 -> "senior", .level == 2 -> "mid", _ -> "junior" } }
```

`alias: expression` names a field. Later operations read the fields the
`select` named, so sort before a `select` by the row's own fields, or after
it by the names it gave.

## Aggregates and groups

An aggregate ends a chain, or is a field of a `select`.

```oql
assignments.where { .role == "dev" }.count()

assignments.sum(.hours)

assignments.select { total: sum(.hours), mean: avg(.hours), most: max(.hours), n: count() }

assignments.project.distinct().count()
```

`group_by` then `select`. A key `.role` is named `role`; a key through a
path is named by its last name unless you name it; any other expression
must be named.

```oql
assignments.group_by { .role }.select { role, people: count(), hours: sum(.hours) }.sort { .hours desc }

assignments.group_by { project: .project.name }.having { sum(.hours) > 20 }.select { project, hours: sum(.hours) }

assignments.group_by { .role }.select { role, top: sort { .hours desc }.limit(2).select { id, hours } }
```

Of no rows, `count` and `sum` are 0, and `avg`, `min`, `max` are `null`.

## A query inside a query

Inside a nested query, `.` is the nested row and the outer collection's
name is the outer row.

```oql
people.select { name, busier: people.where { .level > people.level }.count() }

assignments.where { .hours > assignments.avg(.hours) }.select { id, hours }

people.where { .id in projects.owner }.select { name }
```

## Blocks: names, and one row for every pairing

`collection { entries }` is the same query written as a block. In a block,
`name = value` names a value for each row, and `name in collection` makes
one row for every member, which is how two collections are joined. The
block ends in one result entry, usually `select`.

```oql
people {
  mine = .assignments
  where { mine.count() > 1 }
  select { name, hours: mine.hours.sum() }
}

people {
  a in .assignments
  where { a.hours >= 10 }
  select { name, project: a.project.name, role: a.role }
}

projects {
  p in people
  where { p.level >= 3 && p.id != .owner }
  select { project: name, candidate: p.name }
}
```

`name? in collection` keeps a row that has no member, with the name null:

```oql
people {
  a? in .assignments
  select { name, project: a.project.name }
}
```

Group, sort or limit what a block made by chaining after it:

```oql
people {
  a in .assignments
  group_by { a.role }
  select { role, people: count() }
}.sort { .people desc }.limit(1)
```

## Walk a relationship to any depth

```oql
people.where { .id == "kim" }.walk(.reports).select { name, depth: ._depth }

people.select { name, below: .walk(.reports).count() }

tasks.where { .id == "t1" }.walk(.blocked_by.blocker, 2).select { id }
```

`walk(path)` gives every row reached by following the path one or more
times, each once, nearest first. A second argument limits the steps.

## Vectors: find rows by what they are about

A `Vector(n)` field holds `n` numbers, such as an embedding. A vector is
written as a list of numbers. `nearest` gives the `k` rows whose vector is
nearest the one asked about, nearest first, each with `._score`.

```oql
tasks.nearest { field: .about, query: [1.0, 0.0, 0.0], metric: "cosine", k: 2 }.select { id, score: ._score }

tasks.where { .state == "open" }.nearest { field: .about, query: [1.0, 0.0, 0.0], k: 2 }.select { id }

projects.select { name, near: .tasks.nearest { field: .about, query: [0.0, 1.0, 0.0], k: 1 }.select { id } }
```

- `metric` is `"cosine"` (the default: by direction), `"l2"` (by
  straight-line distance) or `"dot"`. A higher `._score` is nearer.
- A filter before `nearest` searches only the rows that pass it. After a
  relationship, it searches only that row's related rows.
- A row with no vector is never found.

Vectors are also values, with functions of their own: `similarity(a, b)`
(the cosine: 1 the same way, 0 unrelated), `distance(a, b)`, `dot(a, b)`,
`norm(a)`, and `centroid(vectors)` for the mean of several. With no vector
on one side they give `null`.

```oql
tasks.where { similarity(.about, [1.0, 0.0, 0.0]) > 0.9 }.select { id }

tasks.sort { distance(.about, [0.0, 1.0, 0.0]) }.limit(2).select { id }

projects.select { name, middle: centroid(.tasks.about) }

tasks.group_by { .state }.select { state, near: where { similarity(.about, [1.0, 0.0, 0.0]) > 0.5 }.count() }
```

## Writes

```oql
people.insert { id: "dee", name: "Dee", level: 1, manager: "kim" }

people.where { .id == "dee" }.update { level += 1 }

people.where { .id == "dee" }.delete()
```

## Parameters

Values from outside are `$name`, sent beside the query and never pasted
into it: `people.where { .level >= $level }.limit($n)`.

## Coming from SQL

| SQL | OQL |
|---|---|
| `WHERE a = 1 AND b <> 2` | `where { .a == 1 && .b != 2 }` |
| `ORDER BY a DESC, b` | `sort { .a desc, .b }` |
| `JOIN` on a foreign key | a path: `.owner.name`, or backwards `.assignments` |
| `JOIN` on anything else | a block: `x in other  where { x.f == .f }` |
| `LEFT JOIN` | `x? in other` |
| `GROUP BY a` with aggregates | `group_by { .a }.select { a, n: count() }` |
| `HAVING` | `.having { count() > 1 }` between `group_by` and `select` |
| `COUNT(DISTINCT a)` | `.a.distinct().count()` |
| `EXISTS (subquery)` | `.relationship.where { .. }.exists()` |
| `IN (subquery)` | `.x in other.where { .. }.field` |
| `CASE WHEN .. THEN ..` | `case { cond -> value, _ -> other }` |
| `IS NULL`, `LIKE 'A%'`, `BETWEEN` | `is null`, `like "A%"`, `between a and b` |
| `LIMIT n OFFSET m` | `.offset(m).limit(n)` |
| recursive CTE | `.walk(.relationship)` |
| `ORDER BY embedding <=> $q LIMIT k` | `.nearest { field: .embedding, query: $q, k: k }` |

## Mistakes the database refuses

Each of these is an error, and the error says what to write.

```oql-error
people.where { .name = "Kim" }
```

`=` names a value in a block. Equality is `==`.

```oql-error
people.where { .assignments.hours > 10 }
```

A path through a collection has many values. Write
`.assignments.where { .hours > 10 }.exists()` or reduce it with `sum`,
`max`, `count`.

```oql-error
people.where { .level > "2" }
```

A number is not compared with text.

```oql-error
people.where { count() > 1 }
```

An aggregate needs rows: `.reports.count() > 1`.

```oql-error
people.select { name, boss: .manger.name }
```

A name the schema does not have is an error with the nearest name it does
have. Ask for the catalogue before guessing a name.
