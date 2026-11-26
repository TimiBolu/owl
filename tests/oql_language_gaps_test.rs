//! The forms OQL gained so that a query reads as directly as its SQL or
//! MongoDB would, and the texts that used to be accepted and answered
//! wrongly. Each case is run against the small shop of the completeness
//! matrix, afresh, and held to the rows written beside it.

use owl_db::storage::RustStore;
use owl_db::{execute_oql_file, parse_oql, parse_oql_file, QueryExecutor};
use std::sync::Arc;
use tempfile::TempDir;

const SHOP: &str = r#"
schema customers {
  id: String,
  name: String,
  email: String?,
  city: String,
  joined: DateTime,
  tags: List(String),
  address: { street: String, zip: String },
  referrer: Optional(Ref(customers)) as referred
}
schema products {
  id: String,
  name: String,
  price: Float,
  stock: Integer,
  category: String
}
schema orders {
  id: String,
  customer: Ref(customers),
  status: String,
  placed: DateTime,
  total: Float,
  items: List({ product: Ref(products), qty: Integer, price: Float })
}
customers.insert [
  { id: "c1", name: "Ada", address: { street: "1 Marina", zip: "100001" }, email: "ada@x.io", city: "Lagos", joined: "2024-01-10", tags: ["vip", "early"] },
  { id: "c2", name: "Bob", address: { street: "2 Ring Rd", zip: "00233" }, city: "Accra", joined: "2024-03-02", tags: [], referrer: "c1" },
  { id: "c3", name: "Cy", address: { street: "3 Allen Ave", zip: "100271" }, email: "cy@y.org", city: "Lagos", joined: "2025-06-20", tags: ["vip"], referrer: "c1" },
  { id: "c4", name: "Di", address: { street: "4 Kenyatta Ave", zip: "00100" }, email: "di@x.io", city: "Nairobi", joined: "2025-07-01", tags: ["new"], referrer: "c3" },
  { id: "c5", name: "Eve", address: { street: "5 Oxford St", zip: "00233" }, email: "eve@y.org", city: "Accra", joined: "2026-02-14", tags: [] }
]
products.insert [
  { id: "p1", name: "Pen", price: 2.0, stock: 100, category: "stationery" },
  { id: "p2", name: "Book", price: 12.5, stock: 40, category: "stationery" },
  { id: "p3", name: "Lamp", price: 30.0, stock: 0, category: "home" },
  { id: "p4", name: "Desk", price: 150.0, stock: 5, category: "home" },
  { id: "p5", name: "Mug", price: 8.0, stock: 25, category: "kitchen" }
]
orders.insert [
  { id: "o1", customer: "c1", status: "paid", placed: "2026-01-05", total: 26.5,
    items: [ { product: "p1", qty: 7, price: 2.0 }, { product: "p2", qty: 1, price: 12.5 } ] },
  { id: "o2", customer: "c1", status: "paid", placed: "2026-02-10", total: 150.0,
    items: [ { product: "p4", qty: 1, price: 150.0 } ] },
  { id: "o3", customer: "c2", status: "open", placed: "2026-02-11", total: 60.0,
    items: [ { product: "p3", qty: 2, price: 30.0 } ] },
  { id: "o4", customer: "c3", status: "paid", placed: "2026-03-01", total: 42.0,
    items: [ { product: "p3", qty: 1, price: 30.0 }, { product: "p1", qty: 6, price: 2.0 } ] },
  { id: "o5", customer: "c3", status: "cancelled", placed: "2026-03-02", total: 12.5,
    items: [ { product: "p2", qty: 1, price: 12.5 } ] },
  { id: "o6", customer: "c4", status: "paid", placed: "2026-03-15", total: 25.0,
    items: [ { product: "p2", qty: 2, price: 12.5 } ] }
]
"#;

struct Db {
    _directory: TempDir,
    executor: QueryExecutor<RustStore>,
}

impl Db {
    fn shop() -> Self {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
        let db = Self {
            _directory: directory,
            executor: QueryExecutor::new_with_real_clock(store),
        };
        db.script(SHOP).unwrap();
        db
    }

    /// Run a script for the rows its last statement gives.
    fn script(&self, source: &str) -> Result<String, String> {
        let file = parse_oql_file(source).map_err(|error| error.to_string())?;
        let report = execute_oql_file(&file, &self.executor).map_err(|error| error.to_string())?;
        match report.errors.is_empty() {
            true => Ok(self.shown(&report.rows)),
            false => Err(report.errors.join("; ")),
        }
    }

    fn rows(&self, query: &str) -> Result<String, String> {
        let query = parse_oql(query).map_err(|error| error.to_string())?;
        let rows = self.executor.execute_query(&query).map_err(|error| error.to_string())?;
        Ok(self.shown(&rows))
    }

    /// Rows as a client sees them: a reference is the id it points at.
    fn shown(&self, rows: &[owl_db::Document]) -> String {
        rows.iter()
            .map(|row| {
                let fields = row
                    .fields
                    .iter()
                    .filter(|(name, _)| !name.starts_with('_'))
                    .map(|(name, value)| {
                        let value = self.executor.with_reference_ids(value).unwrap_or_else(|_| value.clone());
                        (name.clone(), value.to_clean_json())
                    })
                    .collect::<serde_json::Map<_, _>>();
                serde_json::Value::Object(fields).to_string()
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Hold each query to its rows, each against a fresh shop.
fn held(cases: &[(&str, &str)]) {
    let mut wrong = Vec::new();
    for (query, rows) in cases {
        match Db::shop().rows(query) {
            Ok(found) if found == *rows => {}
            Ok(found) => wrong.push(format!("{query}\n  gave   {found}\n  wanted {rows}")),
            Err(error) => wrong.push(format!("{query}\n  failed {error}")),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn conditions_read_as_they_are_said() {
    held(&[
        ("orders.where { .total between 25 and 60 }.select { id }",
         "{\"id\":\"o1\"} {\"id\":\"o3\"} {\"id\":\"o4\"} {\"id\":\"o6\"}"),
        ("orders.where { .total not between 25 and 60 }.select { id }",
         "{\"id\":\"o2\"} {\"id\":\"o5\"}"),
        (r#"customers.where { .city not in ["Lagos", "Nairobi"] }.select { id }"#,
         "{\"id\":\"c2\"} {\"id\":\"c5\"}"),
        ("customers.where { .email is null }.select { id }",
         "{\"id\":\"c2\"}"),
        ("customers.where { .email is not null }.select { id }",
         "{\"id\":\"c1\"} {\"id\":\"c3\"} {\"id\":\"c4\"} {\"id\":\"c5\"}"),
        (r#"customers.where { .name like "A%" }.select { id }"#,
         "{\"id\":\"c1\"}"),
        (r#"customers.where { .name like "_y" }.select { id }"#,
         "{\"id\":\"c3\"}"),
        (r#"customers.where { .name not like "A%" }.select { id }"#,
         "{\"id\":\"c2\"} {\"id\":\"c3\"} {\"id\":\"c4\"} {\"id\":\"c5\"}"),
        (r#"customers.where { .name ilike "ADA" }.select { id }"#,
         "{\"id\":\"c1\"}"),
        (r#"customers.where { .tags contains_all ["vip", "early"] }.select { id }"#,
         "{\"id\":\"c1\"}"),
        (r#"customers.where { .tags contains_any ["new", "early"] }.select { id }"#,
         "{\"id\":\"c1\"} {\"id\":\"c4\"}"),
        ("customers.where { .email }.select { id }",
         "{\"id\":\"c1\"} {\"id\":\"c3\"} {\"id\":\"c4\"} {\"id\":\"c5\"}"),
        ("customers.where { !.email }.select { id }",
         "{\"id\":\"c2\"}"),
        (r#"products.where { (.price, .id) > (12.5, "p2") }.sort { .price, .id }.select { id }"#,
         "{\"id\":\"p3\"} {\"id\":\"p4\"}"),
        (r#"products.where { (.category, .stock) == ("home", 5) }.select { id }"#,
         "{\"id\":\"p4\"}"),
        (r#"orders.where { .customer == customers.where { .name == "Bob" } }.select { id }"#,
         "{\"id\":\"o3\"}"),
        (r#"orders.where { .customer != customers.where { .name == "Bob" } }.count()"#,
         "{\"count\":5}"),
        (r#"orders.where { .customer in customers.where { .city == "Lagos" } }.sort { .id }.select { id }"#,
         "{\"id\":\"o1\"} {\"id\":\"o2\"} {\"id\":\"o4\"} {\"id\":\"o5\"}"),
        ("customers.where { .name == 'Ada' }.select { id }",
         "{\"id\":\"c1\"}"),
    ]);
}

#[test]
fn any_all_and_none_are_asked_of_related_rows_and_lists() {
    held(&[
        (r#"customers.where { .orders.any { .status == "paid" } }.sort { .id }.select { id }"#,
         "{\"id\":\"c1\"} {\"id\":\"c3\"} {\"id\":\"c4\"}"),
        (r#"customers.where { .orders.all { .status == "paid" } }.sort { .id }.select { id }"#,
         "{\"id\":\"c1\"} {\"id\":\"c4\"} {\"id\":\"c5\"}"),
        (r#"customers.where { .orders.none { .status == "paid" } }.sort { .id }.select { id }"#,
         "{\"id\":\"c2\"} {\"id\":\"c5\"}"),
        ("orders.where { .items.all { .qty >= 2 } }.sort { .id }.select { id }",
         "{\"id\":\"o3\"} {\"id\":\"o6\"}"),
        ("orders.any { .total > 100 }",
         "{\"exists\":true}"),
    ]);
}

#[test]
fn rows_are_chosen_by_name() {
    held(&[
        ("products.sort { .price }.first(2).select { id }",
         "{\"id\":\"p1\"} {\"id\":\"p5\"}"),
        ("products.max_by { .price }.select { id }",
         "{\"id\":\"p4\"}"),
        ("products.min_by { .price }.select { id }",
         "{\"id\":\"p1\"}"),
        ("products.sample(2).count()",
         "{\"count\":2}"),
        ("products.sort { .price }.last().select { id }",
         "{\"id\":\"p4\"}"),
        ("products.sort { -.price, .id }.limit(2).select { id }",
         "{\"id\":\"p4\"} {\"id\":\"p3\"}"),
        ("customers.sort { .email.nulls_last }.select { id }",
         "{\"id\":\"c1\"} {\"id\":\"c3\"} {\"id\":\"c4\"} {\"id\":\"c5\"} {\"id\":\"c2\"}"),
        ("customers.sort { .email.nulls_first }.select { id }",
         "{\"id\":\"c2\"} {\"id\":\"c1\"} {\"id\":\"c3\"} {\"id\":\"c4\"} {\"id\":\"c5\"}"),
    ]);
}

#[test]
fn values_are_worked_on_directly() {
    held(&[
        (r#"products.where { .id == "p2" }.select { a: round(.price / 3, 2), b: round(.price), c: 7 / 2, d: 7 / 2.0 }"#,
         "{\"a\":4.17,\"b\":13,\"c\":3,\"d\":3.5}"),
        (r#"products.sort { .id }.limit(3).select { id, level: case { .stock == 0 -> "out", .stock < 50 -> "low", _ -> "fine" } }"#,
         "{\"id\":\"p1\",\"level\":\"fine\"} {\"id\":\"p2\",\"level\":\"low\"} {\"id\":\"p3\",\"level\":\"out\"}"),
        (r#"customers.where { .id == "c3" }.select { later: (.joined + days(30)).format("%Y-%m-%d"), earlier: (.joined - weeks(1)).format("%Y-%m-%d"), gap: (.joined - .referrer.joined) / days(1), month: truncate(.joined, "month").format("%Y-%m-%d"), longer: days(2) > hours(47) }"#,
         "{\"earlier\":\"2025-06-13\",\"gap\":527.0,\"later\":\"2025-07-20\",\"longer\":true,\"month\":\"2025-06-01\"}"),
        (r#"customers.where { .id == "c1" }.select { joined: join(.tags, "+"), back: reverse(.tags), at: index_of(.tags, "early"), none: index_of(.tags, "x"), twice: repeat("ab", 2), cut: truncate("abcdef", 3), part: .name[0:2], rest: .tags[1:], head: .tags[:1], names: keys(.address) }"#,
         "{\"at\":1,\"back\":[\"early\",\"vip\"],\"cut\":\"abc\",\"head\":[\"vip\"],\"joined\":\"vip+early\",\"names\":[\"street\",\"zip\"],\"none\":null,\"part\":\"Ad\",\"rest\":[\"early\"],\"twice\":\"abab\"}"),
        (r#"customers.where { .id == "c1" }.select { more: .tags + "gold", fewer: .tags - "vip", fewer2: .tags - ["vip", "early"] }"#,
         "{\"fewer\":[\"early\"],\"fewer2\":[],\"more\":[\"vip\",\"early\",\"gold\"]}"),
        (r#"customers.where { .id == "c1" }.select { id, ...address }"#,
         "{\"id\":\"c1\",\"street\":\"1 Marina\",\"zip\":\"100001\"}"),
        (r#"customers.where { .id == "c1" }.tags"#,
         "{\"tags\":\"vip\"} {\"tags\":\"early\"}"),
        (r#"customers.where { .id == "c1" }.select { shout: .tags.select { upper(.) } }"#,
         "{\"shout\":[\"VIP\",\"EARLY\"]}"),
    ]);
}

#[test]
fn aggregates_take_the_forms_sql_gives_them() {
    held(&[
        (r#"orders.where { .id in ["o1", "o2"] }.sort { .id }.select { id, units: .items.sum(.qty) }"#,
         "{\"id\":\"o1\",\"units\":8} {\"id\":\"o2\",\"units\":1}"),
        (r#"orders.select { buyers: count(distinct .customer), paid: count(where .status == "paid"), paid_total: sum(.total where .status == "paid"), big: count(.total > 50) }"#,
         "{\"big\":2,\"buyers\":4,\"paid\":4,\"paid_total\":243.5}"),
        ("orders.select { a: avg(orders.total) }.limit(1)",
         "{\"a\":52.666666666666664}"),
        ("orders.where { .total > avg(orders.total) }.select { id }",
         "{\"id\":\"o2\"} {\"id\":\"o3\"}"),
        ("orders.select { id, share: round(.total / sum(.total), 2) }",
         "{\"id\":\"o1\",\"share\":0.08} {\"id\":\"o2\",\"share\":0.47} {\"id\":\"o3\",\"share\":0.19} {\"id\":\"o4\",\"share\":0.13} {\"id\":\"o5\",\"share\":0.04} {\"id\":\"o6\",\"share\":0.08}"),
        ("orders.group_by { .status }.select { status, ids: .id }.sort { .status }",
         "{\"ids\":[\"o5\"],\"status\":\"cancelled\"} {\"ids\":[\"o3\"],\"status\":\"open\"} {\"ids\":[\"o1\",\"o2\",\"o4\",\"o6\"],\"status\":\"paid\"}"),
        ("orders.group_by { .customer.city, .status }.select { city, status, n: count() }.sort { .city, .status }",
         "{\"city\":\"Accra\",\"n\":1,\"status\":\"open\"} {\"city\":\"Lagos\",\"n\":1,\"status\":\"cancelled\"} {\"city\":\"Lagos\",\"n\":3,\"status\":\"paid\"} {\"city\":\"Nairobi\",\"n\":1,\"status\":\"paid\"}"),
        ("orders.group_by { .customer }.having { count() > 1 }.select { customer: .customer.id, n: count() }.sort { .customer }",
         "{\"customer\":\"c1\",\"n\":2} {\"customer\":\"c3\",\"n\":2}"),
        ("orders.group_by { .customer }.select { customer: .customer.id, top: max_by { .total }.id }.sort { .customer }",
         "{\"customer\":\"c1\",\"top\":\"o2\"} {\"customer\":\"c2\",\"top\":\"o3\"} {\"customer\":\"c3\",\"top\":\"o4\"} {\"customer\":\"c4\",\"top\":\"o6\"}"),
        ("orders.group_by { rollup(.customer.city, .status) }.select { city, status, n: count(), total: sum(.total) }.sort { .city.nulls_last, .status.nulls_last }",
         "{\"city\":\"Accra\",\"n\":1,\"status\":\"open\",\"total\":60.0} {\"city\":\"Accra\",\"n\":1,\"status\":null,\"total\":60.0} {\"city\":\"Lagos\",\"n\":1,\"status\":\"cancelled\",\"total\":12.5} {\"city\":\"Lagos\",\"n\":3,\"status\":\"paid\",\"total\":218.5} {\"city\":\"Lagos\",\"n\":4,\"status\":null,\"total\":231.0} {\"city\":\"Nairobi\",\"n\":1,\"status\":\"paid\",\"total\":25.0} {\"city\":\"Nairobi\",\"n\":1,\"status\":null,\"total\":25.0} {\"city\":null,\"n\":6,\"status\":null,\"total\":316.0}"),
        ("orders.group_by { cube(.status, big: .total > 50) }.select { status, big, n: count() }.sort { .status.nulls_last, .big.nulls_last }",
         "{\"big\":false,\"n\":1,\"status\":\"cancelled\"} {\"big\":null,\"n\":1,\"status\":\"cancelled\"} {\"big\":true,\"n\":1,\"status\":\"open\"} {\"big\":null,\"n\":1,\"status\":\"open\"} {\"big\":false,\"n\":3,\"status\":\"paid\"} {\"big\":true,\"n\":1,\"status\":\"paid\"} {\"big\":null,\"n\":4,\"status\":\"paid\"} {\"big\":false,\"n\":4,\"status\":null} {\"big\":true,\"n\":2,\"status\":null} {\"big\":null,\"n\":6,\"status\":null}"),
        ("orders.group_by { rollup(.status) }.having { count() > 1 }.select { status, n: count() }.sort { .status.nulls_last }",
         "{\"n\":4,\"status\":\"paid\"} {\"n\":6,\"status\":null}"),
    ]);
}

#[test]
fn rows_are_dealt_into_buckets_and_ranked_by_share() {
    held(&[
        ("orders.sort { .total }.select { id, quarter: ntile(4), half: ntile(2), many: ntile(10), rank: percent_rank(.total), within: round(cume_dist(.total), 2) }",
         "{\"half\":1,\"id\":\"o5\",\"many\":1,\"quarter\":1,\"rank\":0.0,\"within\":0.17} {\"half\":1,\"id\":\"o6\",\"many\":2,\"quarter\":1,\"rank\":0.2,\"within\":0.33} {\"half\":1,\"id\":\"o1\",\"many\":3,\"quarter\":2,\"rank\":0.4,\"within\":0.5} {\"half\":2,\"id\":\"o4\",\"many\":4,\"quarter\":2,\"rank\":0.6,\"within\":0.67} {\"half\":2,\"id\":\"o3\",\"many\":5,\"quarter\":3,\"rank\":0.8,\"within\":0.83} {\"half\":2,\"id\":\"o2\",\"many\":6,\"quarter\":4,\"rank\":1.0,\"within\":1.0}"),
        ("orders.sort { .status, .id }.partition_by { .status }.select { id, t: ntile(2) }",
         "{\"id\":\"o5\",\"t\":1} {\"id\":\"o3\",\"t\":1} {\"id\":\"o1\",\"t\":1} {\"id\":\"o2\",\"t\":1} {\"id\":\"o4\",\"t\":2} {\"id\":\"o6\",\"t\":2}"),
        ("products.sort { .category }.select { id, r: percent_rank(.category), c: cume_dist(.category) }",
         "{\"c\":0.4,\"id\":\"p3\",\"r\":0.0} {\"c\":0.4,\"id\":\"p4\",\"r\":0.0} {\"c\":0.6,\"id\":\"p5\",\"r\":0.5} {\"c\":1.0,\"id\":\"p1\",\"r\":0.75} {\"c\":1.0,\"id\":\"p2\",\"r\":0.75}"),
    ]);
}

#[test]
fn writes_say_what_they_do_and_give_back_their_rows() {
    held(&[
        (r#"orders.delete { .status == "cancelled" }.select { id }"#,
         "{\"id\":\"o5\"}"),
        ("orders.delete().count()",
         "{\"count\":6}"),
        (r#"archive.insert(orders.where { .status == "cancelled" }).select { id, status }"#,
         "{\"id\":\"o5\",\"status\":\"cancelled\"}"),
        (r#"customers.where { .id == "c1" }.update { -email }.select { id, has: exists(.email), email }"#,
         "{\"email\":null,\"has\":false,\"id\":\"c1\"}"),
        (r#"customers.where { .id == "c1" }.update { email: null }.select { id, has: exists(.email) }"#,
         "{\"has\":true,\"id\":\"c1\"}"),
        (r#"customers.where { .id == "c2" }.update { tags += "back" }.select { tags }"#,
         "{\"tags\":[\"back\"]}"),
        (r#"customers.where { .id == "c1" }.update { tags -= "vip" }.select { tags }"#,
         "{\"tags\":[\"early\"]}"),
        (r#"customers.where { .id == "c1" }.update { tags[0]: "gold" }.select { tags }"#,
         "{\"tags\":[\"gold\",\"early\"]}"),
        (r#"orders.where { .id == "o1" }.update { items[0].qty += 1 }.select { qty: .items.qty }"#,
         "{\"qty\":[8,1]}"),
        (r#"orders.where { .id == "o1" }.update { items.where { .price < 5 }.qty += 1 }.select { qty: .items.qty }"#,
         "{\"qty\":[8,1]}"),
        (r#"orders.where { .id == "o3" }.update { customer: customers.where { .name == "Ada" } }.select { id, who: .customer.name }"#,
         "{\"id\":\"o3\",\"who\":\"Ada\"}"),
    ]);
}

#[test]
fn comments_and_a_closing_semicolon_are_allowed() {
    held(&[
        ("orders.count(); -- a trailing comment",
         "{\"count\":6}"),
        ("orders /* in the middle */ .count() // and at the end",
         "{\"count\":6}"),
    ]);
}

/// Text that used to be accepted and answered wrongly, or that is close
/// to a form that works, is refused with a message that says what to write.
#[test]
fn what_cannot_hold_is_refused_and_says_why() {
    let cases: &[(&str, &str)] = &[
        (r#"orders.where { .total > "50" }"#,
         r#"'.total' is a number and "50" is text"#),
        ("orders.where { .id == 1 }",
         "'.id' is text and 1 is a number"),
        (r#"orders.where { .placed > "soon" }"#,
         "text that is not a timestamp"),
        ("orders.where { .status == true }",
         "'.status' is text and true is a yes or no"),
        ("orders.where { .status == open }",
         "'open' is not a field of 'orders'"),
        ("orders.update { status: open }",
         "Text is written in quotes"),
        ("customers.select { y: .joined.year }",
         "year is a function: write .joined.year()"),
        ("customers.select { n: .name.first }",
         "'.name' is text, which has no field 'first'"),
        ("orders.where { .total > avg(.total) }",
         "Name the rows, as in orders.avg(.field)"),
        ("orders.select { n: sum(.items.qty) }",
         "each row holds a list there. For each row's own total, ask the list"),
        ("nosuch.count()",
         "'nosuch' is not defined"),
        (r#"CUSTOMERS.where { .name == "x" }"#,
         "Did you mean 'customers'?"),
        ("orders.all { .total > 1 }",
         "all { .. } is asked of a row's list or of the rows it is related to"),
        ("orders.where { .total roughly 5 }",
         "found 'roughly'"),
        ("orders.group_by { rollup(.status), .customer }.select { status }",
         "take every key of the group_by"),
    ];
    let mut wrong = Vec::new();
    for (query, says) in cases {
        match Db::shop().rows(query) {
            Ok(found) => wrong.push(format!("{query}\n  was accepted and gave {found}")),
            Err(error) if error.contains(says) => {}
            Err(error) => wrong.push(format!("{query}\n  said   {error}\n  wanted {says}")),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A script answers with the rows of its last statement, and a transaction
/// block with the rows of the last statement inside it.
#[test]
fn a_script_and_a_transaction_answer_with_their_last_rows() {
    let block = r#"transaction {
  products.where { .id == "p1" }.update { stock -= 10 }
  products.where { .id == "p2" }.update { stock += 10 }
  products.where { .id in ["p1", "p2"] }.sort { .id }.select { stock }
}"#;
    assert_eq!(Db::shop().script(block).unwrap(), r#"{"stock":90} {"stock":50}"#);
    let script = "products.where { .id == \"p1\" }.update { stock: 1 }\nproducts.where { .stock == 1 }.select { id }";
    assert_eq!(Db::shop().script(script).unwrap(), r#"{"id":"p1"}"#);
    // A block that is dropped answers with nothing but its error.
    let dropped = "transaction {\n  products.where { .id == \"p1\" }.update { stock: 1 }\n  nosuch.count()\n}";
    let db = Db::shop();
    assert!(db.script(dropped).unwrap_err().contains("'nosuch' is not defined"));
    assert_eq!(db.rows(r#"products.where { .id == "p1" }.select { stock }"#).unwrap(), r#"{"stock":100}"#);
}

/// A name nothing was ever written under is a mistake. A collection that
/// was written to and then emptied is still a collection.
#[test]
fn an_emptied_collection_still_answers() {
    let db = Db::shop();
    assert!(db.rows("scraps.count()").unwrap_err().contains("'scraps' is not defined"));
    db.rows(r#"scraps.insert { id: "s1" }"#).unwrap();
    db.rows("scraps.delete()").unwrap();
    assert_eq!(db.rows("scraps.count()").unwrap(), r#"{"count":0}"#);
    assert_eq!(db.rows("orders.delete().count()").unwrap(), r#"{"count":6}"#);
    assert_eq!(db.rows("orders.where { .total > 1 }.count()").unwrap(), r#"{"count":0}"#);
}

/// What stays as it was, on purpose: `||` between two texts is "or", the
/// first that is there, and never joins them; and whole numbers divide to
/// a whole number.
#[test]
fn or_is_not_concatenation_and_whole_numbers_divide_whole() {
    held(&[
        (r#"customers.where { .id == "c1" }.select { a: .name || "x", b: .name + "x", c: 7 / 2, d: 7 / 2.0, e: round(7 / 2.0), f: round(3.14159, 2) }"#,
         r#"{"a":"Ada","b":"Adax","c":3,"d":3.5,"e":4,"f":3.14}"#),
    ]);
}
