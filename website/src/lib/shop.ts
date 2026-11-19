// The small shop every chapter of the language tour runs against. The same rows were loaded
// into a real owl-server to check each query and answer shown on the page.

export type Order = { id: string; customer: string; status: "paid" | "open" | "cancelled"; total: number };

export const CUSTOMERS = [
  { id: "ada", name: "Ada", country: "NG" },
  { id: "bola", name: "Bola", country: "NG" },
  { id: "chen", name: "Chen", country: "SG" },
] as const;

export const ORDERS: Order[] = [
  { id: "o1", customer: "ada", status: "paid", total: 125 },
  { id: "o2", customer: "bola", status: "paid", total: 85 },
  { id: "o3", customer: "ada", status: "open", total: 24 },
  { id: "o4", customer: "chen", status: "paid", total: 52 },
  { id: "o5", customer: "chen", status: "cancelled", total: 40 },
  { id: "o6", customer: "ada", status: "paid", total: 97 },
];

export const nameOf = (id: string) => CUSTOMERS.find((c) => c.id === id)!.name;
export const countryOf = (id: string) => CUSTOMERS.find((c) => c.id === id)!.country;
