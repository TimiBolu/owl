-- The same model, relationally. Lists become child tables keyed by
-- (parent, line), and every reference gets an explicit index, as Owl
-- indexes references itself.

CREATE TABLE orgs (id TEXT PRIMARY KEY, name TEXT NOT NULL, plan TEXT NOT NULL, region TEXT NOT NULL);

CREATE TABLE units (
  id TEXT PRIMARY KEY, org TEXT NOT NULL REFERENCES orgs(id), name TEXT NOT NULL,
  parent TEXT REFERENCES units(id), budget REAL NOT NULL
);
CREATE INDEX units_by_org ON units(org);
CREATE INDEX units_by_parent ON units(parent);

CREATE TABLE people (
  id TEXT PRIMARY KEY, org TEXT NOT NULL REFERENCES orgs(id),
  unit TEXT NOT NULL REFERENCES units(id), manager TEXT REFERENCES people(id),
  name TEXT NOT NULL, level INTEGER NOT NULL, salary REAL NOT NULL,
  hired TEXT NOT NULL, departed TEXT
);
CREATE INDEX people_by_org ON people(org);
CREATE INDEX people_by_unit ON people(unit);
CREATE INDEX people_by_manager ON people(manager);
CREATE INDEX people_by_level ON people(level);
CREATE TABLE people_skills (parent TEXT NOT NULL REFERENCES people(id), line INTEGER NOT NULL, value TEXT NOT NULL, PRIMARY KEY (parent, line));

CREATE TABLE projects (
  id TEXT PRIMARY KEY, org TEXT NOT NULL REFERENCES orgs(id),
  owner TEXT NOT NULL REFERENCES people(id), name TEXT NOT NULL,
  state TEXT NOT NULL, budget REAL NOT NULL, started TEXT NOT NULL
);
CREATE INDEX projects_by_org ON projects(org);
CREATE INDEX projects_by_owner ON projects(owner);
CREATE INDEX projects_by_state ON projects(state);
CREATE TABLE projects_tags (parent TEXT NOT NULL REFERENCES projects(id), line INTEGER NOT NULL, value TEXT NOT NULL, PRIMARY KEY (parent, line));

CREATE TABLE assignments (
  id TEXT PRIMARY KEY, person TEXT NOT NULL REFERENCES people(id),
  project TEXT NOT NULL REFERENCES projects(id), role TEXT NOT NULL, hours INTEGER NOT NULL
);
CREATE INDEX assignments_by_person ON assignments(person);
CREATE INDEX assignments_by_project ON assignments(project);

CREATE TABLE tasks (
  id TEXT PRIMARY KEY, project TEXT NOT NULL REFERENCES projects(id),
  assignee TEXT REFERENCES people(id), parent TEXT REFERENCES tasks(id),
  state TEXT NOT NULL, points INTEGER NOT NULL, opened TEXT NOT NULL, closed TEXT
);
CREATE INDEX tasks_by_project ON tasks(project);
CREATE INDEX tasks_by_assignee ON tasks(assignee);
CREATE INDEX tasks_by_parent ON tasks(parent);
CREATE INDEX tasks_by_state_opened ON tasks(state, opened);
CREATE TABLE tasks_labels (parent TEXT NOT NULL REFERENCES tasks(id), line INTEGER NOT NULL, value TEXT NOT NULL, PRIMARY KEY (parent, line));
CREATE TABLE tasks_worklog (
  parent TEXT NOT NULL REFERENCES tasks(id), line INTEGER NOT NULL,
  at TEXT NOT NULL, author TEXT NOT NULL REFERENCES people(id),
  state TEXT NOT NULL, minutes INTEGER NOT NULL, PRIMARY KEY (parent, line)
);
CREATE INDEX tasks_worklog_by_author ON tasks_worklog(author);

CREATE TABLE links (
  id TEXT PRIMARY KEY, blocker TEXT NOT NULL REFERENCES tasks(id),
  blocked TEXT NOT NULL REFERENCES tasks(id), kind TEXT NOT NULL
);
CREATE INDEX links_by_blocker ON links(blocker);
CREATE INDEX links_by_blocked ON links(blocked);

CREATE TABLE events (
  id TEXT PRIMARY KEY, person TEXT NOT NULL REFERENCES people(id),
  kind TEXT NOT NULL, at TEXT NOT NULL, amount INTEGER NOT NULL
);
CREATE INDEX events_by_person ON events(person);
CREATE INDEX events_by_at ON events(at);
