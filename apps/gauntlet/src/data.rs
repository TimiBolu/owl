//! A deterministic workspace dataset, built to be hard on a query engine.
//!
//! One `people` count always produces the same rows. Tables are generic so
//! one loader serves both engines: a list of objects or of text is part of
//! the row in Owl and a child table in SQLite.

pub enum Cell {
    Text(String),
    Int(i64),
    Float(f64),
    /// An optional field with no value.
    Absent,
    /// A list of text values.
    Texts(Vec<String>),
    /// A list of objects.
    Rows(Vec<Row>),
}

pub type Row = Vec<(&'static str, Cell)>;

pub struct Table {
    pub name: &'static str,
    pub rows: Vec<Row>,
}

/// SplitMix64: small, fast, and identical on every platform.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    fn between(&mut self, low: i64, high: i64) -> i64 {
        low + (self.next() % (high - low + 1) as u64) as i64
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    /// An index below `bound`, heavily favouring the low end: the product
    /// of two uniform draws, so the first tenth is picked about a third of
    /// the time.
    fn skewed(&mut self, bound: usize) -> usize {
        let unit = |random: &mut Self| (random.next() % 1_000_000) as f64 / 1_000_000.0;
        ((unit(self) * unit(self)) * bound as f64) as usize
    }

    fn pick<T: Copy>(&mut self, options: &[T]) -> T {
        options[self.below(options.len())]
    }
}

/// 2025-01-01T00:00:00Z.
const EPOCH: i64 = 1_735_689_600;
const DAY: i64 = 86_400;

fn timestamp(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .expect("generated timestamps are in range")
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

const ORGS: usize = 12;
const PLANS: [&str; 3] = ["free", "team", "enterprise"];
const REGIONS: [&str; 4] = ["africa", "europe", "americas", "asia"];
const SKILLS: [&str; 12] = [
    "rust", "sql", "design", "ops", "data", "mobile", "web", "security", "support", "sales",
    "writing", "research",
];
const TAGS: [&str; 8] = [
    "internal", "client", "research", "migration", "urgent", "legacy", "growth", "compliance",
];
const LABELS: [&str; 8] = ["bug", "feature", "chore", "urgent", "blocked", "docs", "test", "debt"];
const TASK_STATES: [&str; 4] = ["open", "doing", "review", "done"];
const PROJECT_STATES: [&str; 3] = ["active", "paused", "done"];
const ROLES: [&str; 4] = ["lead", "member", "reviewer", "observer"];
const EVENT_KINDS: [&str; 4] = ["login", "commit", "review", "deploy"];
const LINK_KINDS: [&str; 2] = ["blocks", "relates"];

fn text(value: impl Into<String>) -> Cell {
    Cell::Text(value.into())
}

fn some_of(random: &mut Random, options: &[&str], most: usize) -> Cell {
    let mut chosen: Vec<String> = Vec::new();
    for _ in 0..random.below(most + 1) {
        let option = random.pick(options).to_string();
        if !chosen.contains(&option) {
            chosen.push(option);
        }
    }
    Cell::Texts(chosen)
}

/// Every table, in an order that inserts a row after the rows it refers to.
pub fn generate(people_count: usize) -> Vec<Table> {
    let mut random = Random(0x6A75_17E7 ^ people_count as u64);
    let unit_count = (people_count / 20).max(ORGS * 3);
    let project_count = (people_count / 4).max(ORGS * 2);
    let task_count = people_count * 10;

    let orgs: Vec<Row> = (0..ORGS)
        .map(|index| {
            vec![
                ("id", text(format!("g{index:02}"))),
                ("name", text(format!("Org {index}"))),
                ("plan", text(PLANS[index % PLANS.len()])),
                ("region", text(REGIONS[index % REGIONS.len()])),
            ]
        })
        .collect();
    // Tenants are skewed: the first holds about a third of everything.
    let org_of = |random: &mut Random| random.skewed(ORGS);

    // Each unit after an org's first hangs under an earlier unit of the same
    // org, usually a recent one, so the trees are deep and uneven.
    let mut units_in: Vec<Vec<usize>> = vec![Vec::new(); ORGS];
    let mut units = Vec::with_capacity(unit_count);
    for index in 0..unit_count {
        let org = if index < ORGS { index } else { org_of(&mut random) };
        let siblings = &units_in[org];
        let parent = match siblings.len() {
            0 => Cell::Absent,
            count if random.chance(60) => {
                text(format!("u{:05}", siblings[count - 1 - random.below(count.min(2))]))
            }
            count => text(format!("u{:05}", siblings[random.below(count)])),
        };
        units.push(vec![
            ("id", text(format!("u{index:05}"))),
            ("org", text(format!("g{org:02}"))),
            ("name", text(format!("Unit {index}"))),
            ("parent", parent),
            ("budget", Cell::Float(random.between(10_000, 900_000) as f64)),
        ]);
        units_in[org].push(index);
    }

    let mut people_in: Vec<Vec<usize>> = vec![Vec::new(); ORGS];
    let mut people = Vec::with_capacity(people_count);
    for index in 0..people_count {
        let org = if index < ORGS { index } else { org_of(&mut random) };
        let colleagues = &people_in[org];
        let manager = match colleagues.len() {
            0 => Cell::Absent,
            count if random.chance(50) => {
                text(format!("p{:06}", colleagues[count - 1 - random.below(count.min(3))]))
            }
            count => text(format!("p{:06}", colleagues[random.skewed(count)])),
        };
        let hired = EPOCH - random.between(0, 2_000) * DAY;
        let level = 1 + random.skewed(7) as i64;
        people.push(vec![
            ("id", text(format!("p{index:06}"))),
            ("org", text(format!("g{org:02}"))),
            ("unit", text(format!("u{:05}", random.pick(&units_in[org])))),
            ("manager", manager),
            ("name", text(format!("Person {index}"))),
            ("level", Cell::Int(level)),
            (
                "salary",
                Cell::Float((30_000 + level * 9_000 + random.between(0, 400) * 50) as f64),
            ),
            ("hired", text(timestamp(hired))),
            (
                "departed",
                if random.chance(10) {
                    text(timestamp(hired + random.between(30, 900) * DAY))
                } else {
                    Cell::Absent
                },
            ),
            ("skills", some_of(&mut random, &SKILLS, 4)),
        ]);
        people_in[org].push(index);
    }

    let mut project_org = Vec::with_capacity(project_count);
    let projects: Vec<Row> = (0..project_count)
        .map(|index| {
            let org = if index < ORGS { index } else { org_of(&mut random) };
            project_org.push(org);
            vec![
                ("id", text(format!("j{index:05}"))),
                ("org", text(format!("g{org:02}"))),
                ("owner", text(format!("p{:06}", random.pick(&people_in[org])))),
                ("name", text(format!("Project {index}"))),
                ("state", text(random.pick(&PROJECT_STATES))),
                ("budget", Cell::Float(random.between(5, 500) as f64 * 1_000.0)),
                ("started", text(timestamp(EPOCH + random.between(0, 400) * DAY))),
                ("tags", some_of(&mut random, &TAGS, 3)),
            ]
        })
        .collect();

    // Many-to-many, skewed on both sides, and one in five crosses tenants.
    let assignments: Vec<Row> = (0..people_count * 3)
        .map(|index| {
            let project = random.skewed(project_count);
            let person = if random.chance(80) {
                let colleagues = &people_in[project_org[project]];
                colleagues[random.skewed(colleagues.len())]
            } else {
                random.below(people_count)
            };
            vec![
                ("id", text(format!("a{index:07}"))),
                ("person", text(format!("p{person:06}"))),
                ("project", text(format!("j{project:05}"))),
                ("role", text(random.pick(&ROLES))),
                ("hours", Cell::Int(random.between(1, 40))),
            ]
        })
        .collect();

    let tasks: Vec<Row> = (0..task_count)
        .map(|index| {
            let project = random.skewed(project_count);
            let colleagues = &people_in[project_org[project]];
            let state = TASK_STATES[random.skewed(TASK_STATES.len() * 2) % TASK_STATES.len()];
            let opened = EPOCH + random.between(0, 500 * DAY);
            // Most tasks have a short work log; one in fifty has a long one.
            let entries = if random.chance(2) {
                random.between(50, 200)
            } else {
                random.between(0, 4)
            };
            let mut at = opened;
            let worklog = (0..entries)
                .map(|_| {
                    at += random.between(600, 3 * DAY);
                    vec![
                        ("at", text(timestamp(at))),
                        ("author", text(format!("p{:06}", random.pick(colleagues)))),
                        ("state", text(random.pick(&TASK_STATES))),
                        ("minutes", Cell::Int(random.between(5, 480))),
                    ]
                })
                .collect();
            vec![
                ("id", text(format!("t{index:07}"))),
                ("project", text(format!("j{project:05}"))),
                (
                    "assignee",
                    if random.chance(25) {
                        Cell::Absent
                    } else {
                        text(format!("p{:06}", colleagues[random.skewed(colleagues.len())]))
                    },
                ),
                (
                    "parent",
                    if index > 0 && random.chance(40) {
                        text(format!("t{:07}", index - 1 - random.below(index.min(6))))
                    } else {
                        Cell::Absent
                    },
                ),
                ("state", text(state)),
                ("points", Cell::Int(random.pick(&[1, 2, 3, 5, 8, 13]))),
                ("opened", text(timestamp(opened))),
                (
                    "closed",
                    if state == "done" {
                        text(timestamp(opened + random.between(3_600, 60 * DAY)))
                    } else {
                        Cell::Absent
                    },
                ),
                ("labels", some_of(&mut random, &LABELS, 3)),
                ("worklog", Cell::Rows(worklog)),
            ]
        })
        .collect();

    // A dependency graph: mostly short hops back to an earlier task, so
    // chains form, with no cycles.
    let links: Vec<Row> = (0..people_count * 3)
        .map(|index| {
            let blocked = 1 + random.below(task_count - 1);
            let blocker = blocked - 1 - random.below(blocked.min(40));
            vec![
                ("id", text(format!("l{index:07}"))),
                ("blocker", text(format!("t{blocker:07}"))),
                ("blocked", text(format!("t{blocked:07}"))),
                ("kind", text(random.pick(&LINK_KINDS))),
            ]
        })
        .collect();

    // Activity arrives in bursts per person: sessions separated by long gaps.
    let mut clock = vec![EPOCH; people_count];
    let events: Vec<Row> = (0..people_count * 20)
        .map(|index| {
            let person = random.skewed(people_count);
            clock[person] += if random.chance(15) {
                random.between(6 * 3_600, 9 * DAY)
            } else {
                random.between(30, 1_500)
            };
            vec![
                ("id", text(format!("e{index:08}"))),
                ("person", text(format!("p{person:06}"))),
                ("kind", text(random.pick(&EVENT_KINDS))),
                ("at", text(timestamp(clock[person]))),
                ("amount", Cell::Int(random.between(-50, 200))),
            ]
        })
        .collect();

    vec![
        Table { name: "orgs", rows: orgs },
        Table { name: "units", rows: units },
        Table { name: "people", rows: people },
        Table { name: "projects", rows: projects },
        Table { name: "assignments", rows: assignments },
        Table { name: "tasks", rows: tasks },
        Table { name: "links", rows: links },
        Table { name: "events", rows: events },
    ]
}
