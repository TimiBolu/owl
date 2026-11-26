// The shop of tests/completeness_matrix_test.rs as MongoDB documents: the
// id in _id, a reference held as the id it points to.
db = db.getSiblingDB("shop");
db.dropDatabase();
db.customers.insertMany([
  { _id: "c1", name: "Ada", address: { street: "1 Marina", zip: "100001" }, email: "ada@x.io", city: "Lagos", joined: ISODate("2024-01-10"), tags: ["vip", "early"] },
  { _id: "c2", name: "Bob", address: { street: "2 Ring Rd", zip: "00233" }, city: "Accra", joined: ISODate("2024-03-02"), tags: [], referrer: "c1" },
  { _id: "c3", name: "Cy", address: { street: "3 Allen Ave", zip: "100271" }, email: "cy@y.org", city: "Lagos", joined: ISODate("2025-06-20"), tags: ["vip"], referrer: "c1" },
  { _id: "c4", name: "Di", address: { street: "4 Kenyatta Ave", zip: "00100" }, email: "di@x.io", city: "Nairobi", joined: ISODate("2025-07-01"), tags: ["new"], referrer: "c3" },
  { _id: "c5", name: "Eve", address: { street: "5 Oxford St", zip: "00233" }, email: "eve@y.org", city: "Accra", joined: ISODate("2026-02-14"), tags: [] }
]);
db.products.insertMany([
  { _id: "p1", name: "Pen", price: 2.0, stock: 100, category: "stationery" },
  { _id: "p2", name: "Book", price: 12.5, stock: 40, category: "stationery" },
  { _id: "p3", name: "Lamp", price: 30.0, stock: 0, category: "home" },
  { _id: "p4", name: "Desk", price: 150.0, stock: 5, category: "home" },
  { _id: "p5", name: "Mug", price: 8.0, stock: 25, category: "kitchen" }
]);
db.orders.insertMany([
  { _id: "o1", customer: "c1", status: "paid", placed: ISODate("2026-01-05"), total: 26.5, items: [{ product: "p1", qty: 7, price: 2.0 }, { product: "p2", qty: 1, price: 12.5 }] },
  { _id: "o2", customer: "c1", status: "paid", placed: ISODate("2026-02-10"), total: 150.0, items: [{ product: "p4", qty: 1, price: 150.0 }] },
  { _id: "o3", customer: "c2", status: "open", placed: ISODate("2026-02-11"), total: 60.0, items: [{ product: "p3", qty: 2, price: 30.0 }] },
  { _id: "o4", customer: "c3", status: "paid", placed: ISODate("2026-03-01"), total: 42.0, items: [{ product: "p3", qty: 1, price: 30.0 }, { product: "p1", qty: 6, price: 2.0 }] },
  { _id: "o5", customer: "c3", status: "cancelled", placed: ISODate("2026-03-02"), total: 12.5, items: [{ product: "p2", qty: 1, price: 12.5 }] },
  { _id: "o6", customer: "c4", status: "paid", placed: ISODate("2026-03-15"), total: 25.0, items: [{ product: "p2", qty: 2, price: 12.5 }] }
]);
