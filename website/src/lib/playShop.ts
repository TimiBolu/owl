// What the playground starts with and offers to run. The shop is the one the
// language tour draws: the same customers and orders, with the products and
// order lines its first chapter declares.
import { CUSTOMERS, ORDERS } from "./shop";

const PRODUCTS = [
  { id: "kettle", name: "Kettle", price: 45 },
  { id: "mug", name: "Mug", price: 12 },
  { id: "tea", name: "Tea", price: 8 },
  { id: "lamp", name: "Lamp", price: 40 },
];

// What each order was for. The lines add up to the order's total.
const LINES: Record<string, Array<[product: string, qty: number]>> = {
  o1: [["kettle", 1], ["lamp", 2]],
  o2: [["kettle", 1], ["lamp", 1]],
  o3: [["mug", 2]],
  o4: [["lamp", 1], ["mug", 1]],
  o5: [["lamp", 1]],
  o6: [["kettle", 1], ["lamp", 1], ["mug", 1]],
};

const DAYS: Record<string, string> = { o1: "02", o2: "03", o3: "05", o4: "06", o5: "09", o6: "12" };

const rows = <T>(items: readonly T[], write: (item: T) => string) => items.map((item) => `  ${write(item)}`).join(",\n");

export const SHOP = `schema customers { id: String, name: String, country: String }
schema products { id: String, name: String, price: Float }
schema orders {
  id: String,
  customer: Ref(customers),
  status: String,
  total: Float,
  placed: DateTime,
  items: List({ product: Ref(products), qty: Integer })
}

customers.insert [
${rows(CUSTOMERS, (c) => `{ id: "${c.id}", name: "${c.name}", country: "${c.country}" }`)}
]

products.insert [
${rows(PRODUCTS, (p) => `{ id: "${p.id}", name: "${p.name}", price: ${p.price}.0 }`)}
]

orders.insert [
${rows(ORDERS, (o) => {
  const items = LINES[o.id].map(([product, qty]) => `{ product: "${product}", qty: ${qty} }`).join(", ");
  return `{ id: "${o.id}", customer: "${o.customer}", status: "${o.status}", total: ${o.total}.0, placed: "2026-03-${DAYS[o.id]}T10:00:00Z", items: [${items}] }`;
})}
]`;

export const SCHEMA = SHOP.slice(0, SHOP.indexOf("\n\ncustomers.insert"));

export type Sample = { name: string; says: string; code: string };

export const SAMPLES: Sample[] = [
  {
    name: "Ask",
    says: "A query is a chain of small steps.",
    code: `orders
  .where { .status == "paid" }
  .sort { .total.desc }
  .limit(3)
  .select { id, total }`,
  },
  {
    name: "Follow",
    says: "Step through a reference with a dot. No join.",
    code: `orders
  .where { .customer.country == "NG" }
  .sort { .id.asc }
  .select { id, who: .customer.name, total }`,
  },
  {
    name: "Turn back",
    says: "Walk the same reference the other way.",
    code: `customers.sort { .id.asc }.select {
  name,
  orders: .orders.count(),
  spent: .orders
    .where { .status == "paid" }
    .sum(.total)
}`,
  },
  {
    name: "Group",
    says: "Sort rows into piles, then measure each pile.",
    code: `orders
  .group_by { country: .customer.country }
  .select { country, orders: count(), total: sum(.total) }
  .sort { .total.desc }`,
  },
  {
    name: "Inside a list",
    says: "A list in a row is a collection too.",
    code: `orders.items
  .group_by { product: .product.name }
  .select { product, sold: sum(.qty) }
  .sort { .sold.desc }`,
  },
  {
    name: "So far",
    says: "Each row can see the rows before it.",
    code: `orders
  .where { .status == "paid" }
  .sort { .placed.asc }
  .select {
    id,
    total,
    running: so_far().total.sum(),
    number: row_number()
  }`,
  },
  {
    name: "Name it",
    says: "A function is kept in the database, like the data.",
    code: `fn size(total) {
  if total >= 90 then "big" else if total >= 50 then "medium" else "small"
}

orders
  .group_by { size: size(.total) }
  .select { size, orders: count() }
  .sort { .orders.desc }`,
  },
  {
    name: "Write",
    says: "Add a row, then read it back through its customer.",
    code: `orders.upsert on id {
  id: "o7", customer: "bola", status: "open", total: 20.0,
  placed: "2026-03-14T10:00:00Z",
  items: [{ product: "mug", qty: 1 }, { product: "tea", qty: 1 }]
}

customers.where { .id == "bola" }.select {
  name,
  orders: .orders.sort { .id.asc }.select { id, status, total }
}`,
  },
];
