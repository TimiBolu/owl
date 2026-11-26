-- The shop of tests/completeness_matrix_test.rs, kept the relational way:
-- tags is text[], address is jsonb, and an order's items are rows.
SET timezone = 'UTC';
CREATE TABLE customers (
  id text PRIMARY KEY, name text NOT NULL, email text, city text NOT NULL,
  joined timestamptz NOT NULL, tags text[] NOT NULL, address jsonb NOT NULL,
  referrer text REFERENCES customers (id)
);
CREATE TABLE products (
  id text PRIMARY KEY, name text, price double precision, stock integer, category text
);
CREATE TABLE orders (
  id text PRIMARY KEY, customer text REFERENCES customers (id), status text,
  placed timestamptz, total double precision
);
CREATE TABLE order_items (
  order_id text REFERENCES orders (id) ON DELETE CASCADE, position integer,
  product text REFERENCES products (id), qty integer, price double precision,
  PRIMARY KEY (order_id, position)
);
INSERT INTO customers VALUES
  ('c1', 'Ada', 'ada@x.io', 'Lagos', '2024-01-10', '{vip,early}', '{"street": "1 Marina", "zip": "100001"}', NULL),
  ('c2', 'Bob', NULL, 'Accra', '2024-03-02', '{}', '{"street": "2 Ring Rd", "zip": "00233"}', 'c1'),
  ('c3', 'Cy', 'cy@y.org', 'Lagos', '2025-06-20', '{vip}', '{"street": "3 Allen Ave", "zip": "100271"}', 'c1'),
  ('c4', 'Di', 'di@x.io', 'Nairobi', '2025-07-01', '{new}', '{"street": "4 Kenyatta Ave", "zip": "00100"}', 'c3'),
  ('c5', 'Eve', 'eve@y.org', 'Accra', '2026-02-14', '{}', '{"street": "5 Oxford St", "zip": "00233"}', NULL);
INSERT INTO products VALUES
  ('p1', 'Pen', 2.0, 100, 'stationery'), ('p2', 'Book', 12.5, 40, 'stationery'),
  ('p3', 'Lamp', 30.0, 0, 'home'), ('p4', 'Desk', 150.0, 5, 'home'), ('p5', 'Mug', 8.0, 25, 'kitchen');
INSERT INTO orders VALUES
  ('o1', 'c1', 'paid', '2026-01-05', 26.5), ('o2', 'c1', 'paid', '2026-02-10', 150.0),
  ('o3', 'c2', 'open', '2026-02-11', 60.0), ('o4', 'c3', 'paid', '2026-03-01', 42.0),
  ('o5', 'c3', 'cancelled', '2026-03-02', 12.5), ('o6', 'c4', 'paid', '2026-03-15', 25.0);
INSERT INTO order_items VALUES
  ('o1', 1, 'p1', 7, 2.0), ('o1', 2, 'p2', 1, 12.5), ('o2', 1, 'p4', 1, 150.0),
  ('o3', 1, 'p3', 2, 30.0), ('o4', 1, 'p3', 1, 30.0), ('o4', 2, 'p1', 6, 2.0),
  ('o5', 1, 'p2', 1, 12.5), ('o6', 1, 'p2', 2, 12.5);
