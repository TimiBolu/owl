-- Tables some statements take as given: the ones a row's own Owl script
-- declares before the statement the page shows.
CREATE TABLE jobs (id text PRIMARY KEY, state text, n integer);
INSERT INTO jobs VALUES ('j1', 'open', 1), ('j2', 'open', 2);
CREATE TABLE places (id text PRIMARY KEY, address jsonb);
CREATE TABLE logins (id text PRIMARY KEY, email text);
CREATE TABLE names (id text PRIMARY KEY, name text);
CREATE TABLE posts (id text PRIMARY KEY, tags text[]);
CREATE TABLE sessions (id text PRIMARY KEY, made timestamptz);
CREATE TABLE incoming (LIKE products);
CREATE TABLE archive (LIKE orders);
CREATE FUNCTION touch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$;
