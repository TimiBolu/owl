
# Owl 🦉

**Owl** is a modern, declarative query language designed for seamless data manipulation and retrieval. Built for simplicity and performance, Owl combines the expressiveness of SQL with the flexibility of NoSQL, powered by a Rust-based executor that leverages sled for persistent storage.

Whether you're querying tasks, analyzing geospatial data, or managing complex relationships, Owl’s intuitive syntax and powerful operators make data work feel effortless.

---

## Why Owl?

* **Simple Syntax**: Write queries that read like natural language, reducing boilerplate compared to SQL.
* **Flexible Schemas**: Support for `String`, `Map`, `List`, `GeoPoint`, and more, with `::index` and `::timeseries` for optimization.
* **High Performance**: Rust executor with index-aware lookups and parallel batch processing.
* **Relationship-Friendly**: Native `List<Ref>` support for joins without the clutter of SQL’s `LEFT JOIN LATERAL`.
* **Extensible**: Custom functions and operators like `map`, `each`, and `with` for advanced transformations.

Owl is ideal for developers building data-driven applications, from task management systems to geospatial analytics platforms.

---

## Features

* **Declarative Queries**: Use `where`, `select`, `map`, and `each` for intuitive data operations.
* **Rich Data Types**: Handle `String`, `Int8`, `DateTime`, `Bool`, `Map`, `List`, and `GeoPoint` with type safety.
* **Optimized Execution**: Indexes and timeseries support for fast filtering and time-based queries.
* **Relationship Management**: Seamless one-to-many relationships with `List<Ref>` fields.
* **Parallel Processing**: Batch operations with optional parallelism for large datasets.
* **Persistent Storage**: Backed by `sled`, a lightweight embedded database.

---

## Installation

### Prerequisites

* Rust (stable, 1.65+)
* Cargo
* Git

### Steps

**Clone the Repository:**

```bash
git clone https://github.com/TimiBolu/owl.git
cd owl
```

**Build the Project:**

```bash
cargo build --release
```

**Run the CLI (optional, if implemented):**

```bash
cargo run --bin owl-cli
```

**Add as Dependency (for library usage):**

Add to your `Cargo.toml`:

```toml
[dependencies]
owl = { git = "https://github.com/TimiBolu/owl.git" }
```

---

## Usage

### Defining Schemas

Define schemas with fields, types, and optimizations like `::index` and `::timeseries`.

```rust
schema TeamTask {
    id: String,
    name: String,
    workspaceId: String,
    labels: List<Ref<TeamTaskLabel>>,
    archivedTime: DateTime?
}::index {
    .id,
    .workspaceId,
    .archivedTime
}

schema TeamTaskLabel {
    taskId: Ref<TeamTask>,
    labelId: String
}::index {
    .taskId,
    .labelId
}
```

---

### Querying Data

Fetch tasks with related labels, filtering by workspace and status.

```rust
fn get_team_tasks(userId: String, workspaceId: String, labelId: String) {
    TeamTask::where {
        .workspaceId == workspaceId,
        .archivedTime == null,
        .id in (
            TeamTaskLabel::where {
                .labelId == labelId
            }::select { taskId }
        )
    }::select {
        id,
        name,
        labels: .labels::select { labelId }::default { "[]" }
    }
}
```

**This query:**

* Filters tasks by `workspaceId` and non-archived status.
* Ensures tasks have the specified `labelId`.
* Returns `id`, `name`, and a JSON array of `labelId`s.

---

### Advanced Example

Update nested task preferences with conditional logic:

```rust
fn update_task_preferences(taskId: String) {
    TeamTask::findById { taskId }::update {
        preference: .preference with {
            displayStatus: if .preference.displayStatus == "" then "shown" else "hidden"
        }
    }::batch { size: 1000, parallel: 4 }
}
```

**This:**

* Finds a task by ID.
* Updates the nested preference map.
* Processes in parallel batches for efficiency.

---

## Getting Started

### Set Up a Database

Initialize a sled database and load your schemas.

```rust
use owl::execute;
use sled::Db;

let db = sled::open("owl_db").unwrap();
let program = owl::parse(r#"
    schema TeamTask { ... }
    fn get_team_tasks(...) { ... }
"#).unwrap();
let result = execute(program, &db).unwrap();
```

### Run Queries

Use the Owl CLI or embed queries in your Rust application.

### Explore Examples

Check the `/examples` directory for more use cases, including geospatial queries and timeseries analysis.

---

## Contributing

We welcome contributions! Here’s how to get involved:

* **Fork the Repository.**

* **Create a Branch:**

```bash
git checkout -b feature/your-feature
```

* **Submit a Pull Request:**

  * Ensure tests pass (`cargo test`).
  * Follow the code style guide.
  * Describe your changes clearly.

* **Report Issues:** Use the issue tracker for bugs or feature requests.

---

## Roadmap

* Transaction support with `sys::transaction`.
* Enhanced CLI for interactive querying.
* Integration with other databases (e.g., SQLite, PostgreSQL).
* Visual query builder for non-coders.

---

## License

**Owl** is licensed under the MIT License.

---

## Contact

* GitHub: [https://github.com/TimiBolu/owl](https://github.com/TimiBolu/owl)
* Email: [adesinatim@gmail.com](mailto:adesinatim@gmail.com)
* Discord: Join our community server

---

> **Owl: Query with clarity, execute with speed.**
