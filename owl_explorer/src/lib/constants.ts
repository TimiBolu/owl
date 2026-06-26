
export const SCHEMA_TYPE_OPTIONS = [
    "String",
    "Integer",
    "Float",
    "Boolean",
    "DateTime",
    "Object",
    "Vector(128)",
    "Ref(users)",
    "List(String)",
    "List(Object)",
    "Optional(String)",
];

export const QUERY_SNIPPETS: Array<{ label: string; query: string }> = [
    {
        label: "Batch insert",
        query: `users.insert [
  { id: "u10", name: "Kira", profile: { city: "Lagos", prefs: { alerts: true } }, tags: ["new"] },
  { id: "u11", name: "Noah", profile: { city: "Abuja", prefs: { alerts: false } }, tags: ["pro", "beta"] }
]`,
    },
    {
        label: "Ref + aggregate",
        query: `orders.group_by { .user }.select {
  user,
  total_orders: count(),
  total_amount: sum(.amount)
}`,
    },
    {
        label: "Nested access",
        query: `users.where { .profile.prefs.alerts == true }.select {
  id,
  name,
  city: .profile.city
}`,
    },
];
