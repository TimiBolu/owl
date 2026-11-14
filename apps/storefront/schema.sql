-- The same model, relationally. Line items become their own table, and
-- every reference Owl indexes automatically gets an explicit index here, so
-- neither engine is asked to scan for a relationship the other has indexed.

CREATE TABLE customers (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL UNIQUE,
  tier TEXT NOT NULL, country TEXT NOT NULL, joined TEXT NOT NULL,
  city TEXT NOT NULL, zip TEXT NOT NULL
);
CREATE INDEX customers_by_country ON customers(country);
CREATE INDEX customers_by_tier ON customers(tier);

CREATE TABLE categories (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, parent TEXT REFERENCES categories(id)
);
CREATE INDEX categories_by_parent ON categories(parent);

CREATE TABLE products (
  id TEXT PRIMARY KEY, name TEXT NOT NULL,
  category TEXT NOT NULL REFERENCES categories(id),
  price REAL NOT NULL CHECK (price >= 0), stock INTEGER NOT NULL CHECK (stock >= 0)
);
CREATE INDEX products_by_category ON products(category);

CREATE TABLE orders (
  id TEXT PRIMARY KEY, customer TEXT NOT NULL REFERENCES customers(id),
  status TEXT NOT NULL, placed TEXT NOT NULL, total REAL NOT NULL
);
CREATE INDEX orders_by_customer ON orders(customer);
CREATE INDEX orders_by_status_placed ON orders(status, placed);
CREATE INDEX orders_by_placed ON orders(placed);

CREATE TABLE order_items (
  order_id TEXT NOT NULL REFERENCES orders(id), line INTEGER NOT NULL,
  product TEXT NOT NULL REFERENCES products(id),
  qty INTEGER NOT NULL, price REAL NOT NULL,
  PRIMARY KEY (order_id, line)
);
CREATE INDEX order_items_by_product ON order_items(product);

CREATE TABLE payments (
  id TEXT PRIMARY KEY, order_id TEXT NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
  method TEXT NOT NULL, amount REAL NOT NULL, paid_at TEXT NOT NULL
);
CREATE INDEX payments_by_order ON payments(order_id);
CREATE INDEX payments_by_paid_at ON payments(paid_at);

CREATE TABLE reviews (
  id TEXT PRIMARY KEY, product TEXT NOT NULL REFERENCES products(id),
  customer TEXT NOT NULL REFERENCES customers(id),
  rating INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
  body TEXT NOT NULL, written TEXT NOT NULL
);
CREATE INDEX reviews_by_product ON reviews(product);
CREATE INDEX reviews_by_customer ON reviews(customer);

CREATE TABLE tickets (
  id TEXT PRIMARY KEY, customer TEXT NOT NULL REFERENCES customers(id),
  order_id TEXT REFERENCES orders(id), status TEXT NOT NULL,
  priority INTEGER NOT NULL, opened TEXT NOT NULL
);
CREATE INDEX tickets_by_customer ON tickets(customer);
CREATE INDEX tickets_by_order ON tickets(order_id);
