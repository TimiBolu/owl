use owl_db::storage::{DocumentStore, RustStore};
use owl_db::{
    execute_oql_file, parse_oql, parse_oql_file, parse_oql_with_schema, Document, Expression,
    FieldType, Operation, QueryExecutor, Schema, SchemaRegistry, StatementResult, Value,
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn scoped_and_chained_forms_lower_to_the_same_query() {
    let chained = parse_oql(
        r#"
        stores
          .where { .region == "north" }
          .sort { .name.asc }
          .select { id, name }
        "#,
    )
    .unwrap();

    let scoped = parse_oql(
        r#"
        stores {
          where { .region == "north" }
          sort { .name.asc }
          select { id, name }
        }
        "#,
    )
    .unwrap();

    assert_eq!(scoped, chained);
}

#[test]
fn scoped_query_locals_lower_into_the_canonical_expression_tree() {
    let query = parse_oql(
        r#"
        stores {
          returns = store_returns.where { .store.id == stores.id }
          avg_return = returns.avg(.return_amt)

          where { .region == "north" }
          select {
            id,
            return_count: returns.count(),
            average_return: avg_return
          }
        }
        "#,
    )
    .unwrap();

    let Operation::Select(select) = query.operations.last().unwrap() else {
        panic!("scoped query must lower to a final select");
    };

    assert!(matches!(
        select.fields[1].expression,
        Expression::QueryExpression { .. }
    ));
    assert!(matches!(
        select.fields[2].expression,
        Expression::QueryExpression { .. }
    ));
}

#[test]
fn scoped_bindings_cannot_shadow_or_be_redefined() {
    let shadow = parse_oql(
        r#"
        stores {
          stores = other_stores
          select { id }
        }
        "#,
    )
    .unwrap_err()
    .to_string();
    assert!(shadow.contains("cannot shadow"));

    let duplicate = parse_oql(
        r#"
        stores {
          returns = store_returns
          returns = other_returns
          select { id }
        }
        "#,
    )
    .unwrap_err()
    .to_string();
    assert!(duplicate.contains("already defined"));
}

#[test]
fn scoped_bindings_cannot_be_used_before_they_are_defined() {
    // Defined later in the block: the earlier use would otherwise be lowered
    // without the local and quietly read a collection named `paid`.
    let defined_later = parse_oql(
        r#"
        customers {
          spent = paid.sum(.total)
          paid = .orders.where { .status == "paid" }
          select { name, spent }
        }
        "#,
    )
    .unwrap_err()
    .to_string();
    assert!(
        defined_later.contains("Scoped binding 'paid' is used before it is defined"),
        "{defined_later}"
    );

    // An earlier query transform counts as a use too.
    let used_in_where = parse_oql(
        r#"
        wallets {
          where { .balance >= minimum }
          minimum = 50
          select { id }
        }
        "#,
    )
    .unwrap_err()
    .to_string();
    assert!(
        used_in_where.contains("Scoped binding 'minimum' is used before it is defined"),
        "{used_in_where}"
    );

    // A stored field, a key and a definition in terms of the collection of
    // the same name are not uses of the local.
    parse_oql(
        r#"
        customers {
          where { .paid == true }
          labels = { paid: "yes" }
          paid = paid.where { .customer.id == customers.id }
          select { name, paid: paid.count() }
        }
        "#,
    )
    .unwrap();
}

#[test]
fn a_scoped_name_that_is_never_defined_is_rejected_against_a_schema() {
    // Without schemas a bare name may be a collection or a script binding,
    // so only a parse that knows the schemas can reject it.
    let registry = SchemaRegistry::new();
    registry
        .register(Schema::new(
            "customers".into(),
            HashMap::from([
                ("id".into(), FieldType::String),
                ("name".into(), FieldType::String),
            ]),
        ))
        .unwrap();
    let never_defined = parse_oql_with_schema(
        r#"
        customers {
          spent = paid.sum(.total)
          select { name, spent }
        }
        "#,
        registry,
    )
    .unwrap_err()
    .to_string();
    assert!(never_defined.contains("'paid' not found"), "{never_defined}");
}

#[test]
fn shorthand_scoped_locals_in_select_are_keyed_by_their_names() {
    let query = parse_oql(
        r#"
        customers {
          paid = .orders.where { .status == "paid" }
          spent = paid.sum(.total)
          select { name, orders: paid.count(), spent, paid, total: spent }
        }
        "#,
    )
    .unwrap();
    let Operation::Select(select) = query.operations.last().unwrap() else {
        panic!("scoped query must lower to a final select");
    };
    let aliases: Vec<Option<&str>> = select
        .fields
        .iter()
        .map(|field| field.alias.as_deref())
        .collect();
    assert_eq!(
        aliases,
        vec![None, Some("orders"), Some("spent"), Some("paid"), Some("total")]
    );

    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("shorthand.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let setup = parse_oql_file(
        r#"
        schema customers {
          id: String,
          name: String
        }

        schema orders {
          id: String,
          customer: Ref(customers),
          status: String,
          total: Float
        }

        customers.insert { id: "c1", name: "Ada" }
        orders.insert { id: "o1", customer: "c1", status: "paid", total: 10.0 }
        orders.insert { id: "o2", customer: "c1", status: "paid", total: 5.5 }
        orders.insert { id: "o3", customer: "c1", status: "open", total: 99.0 }
        "#,
    )
    .unwrap();
    let setup_result = execute_oql_file(&setup, &executor).unwrap();
    assert!(setup_result.errors.is_empty(), "{:?}", setup_result.errors);

    // Two shorthand locals in one select used to collide on `computed`.
    let rows = executor
        .execute_query(
            &parse_oql(
                r#"
                customers {
                  paid = .orders.where { .status == "paid" }
                  spent = paid.sum(.total)
                  paid_count = paid.count()
                  select { name, spent, paid_count }
                }
                "#,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_field("name"), Some(&Value::String("Ada".into())));
    assert_eq!(rows[0].get_field("spent"), Some(&Value::Float(15.5)));
    assert_eq!(rows[0].get_field("paid_count"), Some(&Value::Integer(2)));
    assert_eq!(rows[0].get_field("computed"), None);
}

#[test]
fn a_name_that_is_never_defined_is_an_error_when_the_query_runs() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("undefined.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let setup = parse_oql_file(
        r#"
        schema customers {
          id: String,
          name: String
        }

        schema orders {
          id: String,
          customer: Ref(customers),
          status: String,
          total: Float
        }

        customers.insert { id: "c1", name: "Ada" }
        orders.insert { id: "o1", customer: "c1", status: "paid", total: 10.0 }
        notes.insert { id: "n1", customer: "c1" }
        "#,
    )
    .unwrap();
    let setup_result = execute_oql_file(&setup, &executor).unwrap();
    assert!(setup_result.errors.is_empty(), "{:?}", setup_result.errors);

    // Parsed without schemas, as the server parses: `paid` may still be a
    // collection or a script binding, so only running the query can tell.
    let never_defined = r#"
        customers {
          spent = paid.sum(.total)
          select { name, spent }
        }
    "#;
    let error = executor
        .execute_query(&parse_oql(never_defined).unwrap())
        .unwrap_err()
        .to_string();
    assert!(error.contains("'paid' is not defined"), "{error}");

    // The same through a script, and in chain form.
    let script = parse_oql_file(never_defined).unwrap();
    let result = execute_oql_file(&script, &executor).unwrap();
    assert!(
        result.errors.iter().any(|error| error.contains("'paid' is not defined")),
        "{:?}",
        result.errors
    );
    let chained = executor
        .execute_query(&parse_oql("customers.select { name, spent: paid.sum(.total) }").unwrap())
        .unwrap_err()
        .to_string();
    assert!(chained.contains("'paid' is not defined"), "{chained}");

    // A near miss is pointed at the collection it resembles.
    let misspelt = executor
        .execute_query(&parse_oql("customers.select { name, n: order.count() }").unwrap())
        .unwrap_err()
        .to_string();
    assert!(misspelt.contains("Did you mean 'orders'?"), "{misspelt}");

    // Names that are defined still work: a script binding, a collection
    // with a schema but no rows yet, and one with rows but no schema.
    let defined = parse_oql_file(
        r#"
        schema refunds {
          id: String,
          total: Float
        }
        paid = orders.where { .status == "paid" }
        customers.select { name, spent: paid.sum(.total) }
        customers.select { name, refunded: refunds.sum(.total) }
        customers.select { name, notes: notes.count() }
        "#,
    )
    .unwrap();
    let result = execute_oql_file(&defined, &executor).unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);

    // A collection nobody declared is still empty when it is the one
    // being queried, not an error.
    let rows = executor
        .execute_query(&parse_oql("nobody.where { .id == \"x\" }").unwrap())
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn function_locals_cannot_be_used_before_they_are_defined() {
    let out_of_order = parse_oql_file(
        r#"
        fn spread(values) {
          width = top - values.min()
          top = values.max()
          width
        }
        "#,
    )
    .unwrap_err()
    .to_string();
    assert!(
        out_of_order.contains("local name 'top' in function 'spread' is used before it is defined"),
        "{out_of_order}"
    );

    parse_oql_file(
        r#"
        fn spread(values) {
          top = values.max()
          width = top - values.min()
          width
        }
        "#,
    )
    .unwrap();
}

#[test]
fn scoped_query_requires_one_final_select() {
    let missing = parse_oql(
        r#"
        stores {
          where { .region == "north" }
        }
        "#,
    )
    .unwrap_err()
    .to_string();
    assert!(missing.contains("must end in a select or a terminal operation"));

    let not_final = parse_oql(
        r#"
        stores {
          select { id }
          limit 10
        }
        "#,
    )
    .unwrap_err()
    .to_string();
    assert!(not_final.contains("select must be the final"));
}

#[test]
fn scoped_locals_are_lowered_before_schema_validation() {
    let registry = SchemaRegistry::new();
    registry
        .register(Schema::new(
            "wallets".into(),
            HashMap::from([
                ("id".into(), FieldType::String),
                ("balance".into(), FieldType::Integer),
            ]),
        ))
        .unwrap();
    let query = parse_oql_with_schema(
        r#"
        wallets {
          minimum = 50
          where { .balance >= minimum }
          select { id, adjusted: .balance + minimum }
        }
        "#,
        registry,
    )
    .unwrap();
    let Operation::Where(where_block) = &query.operations[0] else {
        panic!("first operation must be where");
    };
    assert!(matches!(
        where_block.condition,
        owl_db::Condition::Comparison {
            right: Expression::Literal(owl_db::Literal::Integer(50)),
            ..
        }
    ));
}

#[test]
fn null_missing_and_broken_reference_predicates_remain_distinct() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("states.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store.clone());

    let present = Document::with_id(
        "present".into(),
        "users".into(),
        owl_db::Fields::from_iter([("nickname".into(), Value::String("Ada".into()))]),
    );
    let explicit_null = Document::with_id(
        "null".into(),
        "users".into(),
        owl_db::Fields::from_iter([("nickname".into(), Value::Null)]),
    );
    let missing = Document::with_id("missing".into(), "users".into(), owl_db::Fields::default());
    let broken = Document::with_id(
        "broken".into(),
        "users".into(),
        owl_db::Fields::from_iter([
            (
                "team".into(),
                Value::Reference(owl_db::document::ObjRef::from_parts(99, 42, 1)),
            ),
            ("nickname".into(), Value::String("Broken Ref User".into())),
        ]),
    );

    for document in [&present, &explicit_null, &missing, &broken] {
        store.put_document(document).unwrap();
    }
    let run = |source: &str| {
        let query = parse_oql(source).unwrap();
        executor.execute_query(&query).unwrap()
    };

    assert_eq!(run("users.where { exists(.nickname) }").len(), 3);
    assert_eq!(run("users.where { is_null(.nickname) }")[0].id, "null");
    assert_eq!(
        run("users.where { is_missing(.nickname) }")[0].id,
        "missing"
    );
    assert_eq!(run("users.where { is_broken_ref(.team) }")[0].id, "broken");
    let ordinary_null_matches = run("users.where { .nickname == null }");
    assert_eq!(ordinary_null_matches.len(), 2);
    assert!(ordinary_null_matches.iter().any(|doc| doc.id == "null"));
    assert!(ordinary_null_matches.iter().any(|doc| doc.id == "missing"));
}

#[test]
fn array_fields_use_the_same_collection_operations_as_top_level_queries() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("arrays.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store.clone());

    let items = vec![
        Value::Object(owl_db::Fields::from_iter([
            ("sku".into(), Value::String("book-1".into())),
            ("category".into(), Value::String("books".into())),
            ("price".into(), Value::Integer(35)),
        ])),
        Value::Object(owl_db::Fields::from_iter([
            ("sku".into(), Value::String("game-1".into())),
            ("category".into(), Value::String("games".into())),
            ("price".into(), Value::Integer(70)),
        ])),
        Value::Object(owl_db::Fields::from_iter([
            ("sku".into(), Value::String("book-2".into())),
            ("category".into(), Value::String("books".into())),
            ("price".into(), Value::Integer(55)),
        ])),
    ];
    store
        .put_document(&Document::with_id(
            "order-1".into(),
            "orders".into(),
            owl_db::Fields::from_iter([
                ("items".into(), Value::List(items)),
                ("featured_sku".into(), Value::String("book-2".into())),
                (
                    "profile".into(),
                    Value::Object(owl_db::Fields::from_iter([
                        ("city".into(), Value::String("Lagos".into())),
                        ("country".into(), Value::String("NG".into())),
                    ])),
                ),
            ]),
        ))
        .unwrap();

    let query = parse_oql(
        r#"
        orders.select {
          id,
          books: .items
            .where { .category == "books" }
            .sort { .price.desc }
            .select { sku, price },
          book_count: .items.where { .category == "books" }.count(),
          has_books: .items.where { .category == "books" }.exists(),
          featured_count: .items.where { .sku == orders.featured_sku }.count(),
          first_book: .items
            .where { .category == "books" }
            .sort { .price.desc }
            .select { sku }
            .first(),
          second_item: .items
            .sort { .price.asc }
            .offset(1)
            .limit(1)
            .select { sku },
          profile: .profile.select { city }
        }
        "#,
    )
    .unwrap();
    let rows = executor.execute_query(&query).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_field("book_count"), Some(&Value::Integer(2)));
    assert_eq!(rows[0].get_field("has_books"), Some(&Value::Boolean(true)));
    assert_eq!(
        rows[0].get_field("featured_count"),
        Some(&Value::Integer(1))
    );
    assert_eq!(
        rows[0].get_field("first_book"),
        Some(&Value::Object(owl_db::Fields::from_iter([(
            "sku".into(),
            Value::String("book-2".into())
        )])))
    );
    assert_eq!(
        rows[0].get_field("profile"),
        Some(&Value::Object(owl_db::Fields::from_iter([(
            "city".into(),
            Value::String("Lagos".into())
        )])))
    );
    assert_eq!(
        rows[0].get_field("second_item"),
        Some(&Value::List(vec![Value::Object(owl_db::Fields::from_iter([(
            "sku".into(),
            Value::String("book-2".into())
        )]))]))
    );
    let Value::List(books) = rows[0].get_field("books").unwrap() else {
        panic!("books projection must remain collection-valued");
    };
    assert_eq!(books.len(), 2);
    let Value::Object(first) = &books[0] else {
        panic!("projected book must be an object");
    };
    assert_eq!(first.get("sku"), Some(&Value::String("book-2".into())));

    let scoped = parse_oql(
        r#"
        orders {
          books = .items.where { .category == "books" }
          select {
            id,
            book_count: books.count(),
            books: books.sort { .price.desc }.select { sku, price }
          }
        }
        "#,
    )
    .unwrap();
    let scoped_rows = executor.execute_query(&scoped).unwrap();
    assert_eq!(
        scoped_rows[0].get_field("book_count"),
        Some(&Value::Integer(2))
    );
    assert_eq!(
        scoped_rows[0].get_field("books"),
        rows[0].get_field("books")
    );
}

#[test]
fn declared_references_provide_implicit_reverse_collections() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("reverse.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let setup = parse_oql_file(
        r#"
        schema accounts {
          id: String
        }

        schema companies {
          id: String,
          name: String,
          account: Ref(accounts)
        }

        schema users {
          id: String,
          account: Ref(accounts)
        }

        schema findings {
          id: String,
          company: Ref(companies),
          status: String
        }

        accounts.insert { id: "a1" }
        accounts.insert { id: "a2" }
        companies.insert { id: "c1", name: "Acme", account: "a1" }
        companies.insert { id: "c2", name: "Other", account: "a2" }
        users.insert { id: "u1", account: "a1" }
        findings.insert { id: "f1", company: "c1", status: "open" }
        findings.insert { id: "f2", company: "c1", status: "closed" }
        findings.insert { id: "f3", company: "c2", status: "open" }
        "#,
    )
    .unwrap();
    let setup_result = execute_oql_file(&setup, &executor).unwrap();
    assert!(setup_result.errors.is_empty(), "{:?}", setup_result.errors);

    let query = parse_oql(
        r#"
        companies.sort { .name.asc }.select {
          id,
          open_findings: .findings.where { .status == "open" }.count(),
          finding_ids: .findings.sort { .id.asc }.select { id }
        }
        "#,
    )
    .unwrap();
    let rows = executor.execute_query(&query).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get_field("id"), Some(&Value::String("c1".into())));
    assert_eq!(rows[0].get_field("open_findings"), Some(&Value::Integer(1)));
    assert_eq!(rows[1].get_field("open_findings"), Some(&Value::Integer(1)));
    let Value::List(findings) = rows[0].get_field("finding_ids").unwrap() else {
        panic!("reverse traversal must be collection-valued");
    };
    assert_eq!(findings.len(), 2);

    let deep_reverse = executor
        .execute_query(
            &parse_oql(
                r#"
                users.select {
                  id,
                  company_count: .account.companies.count(),
                  companies: .account.companies.select { id, name }
                }
                "#,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        deep_reverse[0].get_field("company_count"),
        Some(&Value::Integer(1))
    );
}

#[test]
fn script_values_and_queries_share_one_binding_environment() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("bindings.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let file = parse_oql_file(
        r#"
        threshold = 50
        labels = { currency: "USD", region: "west" }
        items = [{ sku: "book", price: 35 }, { sku: "game", price: 70 }, { sku: "desk", price: 90 }]
        expensive = items.where { .price > threshold }
        expensive.sort { .price.asc }.select { sku, price }
        labels.select { currency }
        transaction {
          transaction_limit = 80
          items.where { .price >= transaction_limit }.count()
        }
        "#,
    )
    .unwrap();

    assert!(matches!(
        file.statements[0],
        owl_db::OQLFileStatement::ValueBinding(_)
    ));
    assert!(matches!(
        file.statements[3],
        owl_db::OQLFileStatement::QueryBinding(_)
    ));

    let result = execute_oql_file(&file, &executor).unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let query_results: Vec<usize> = result
        .results
        .iter()
        .filter_map(|result| match result {
            StatementResult::Query(count) => Some(*count),
            _ => None,
        })
        .collect();
    assert_eq!(query_results, vec![2, 1, 1]);
    assert!(result.results.iter().any(
        |result| matches!(result, StatementResult::Transaction(status) if status == "committed")
    ));
}

#[test]
fn nested_and_arithmetic_updates_preserve_shaped_mutation_semantics() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("updates.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store);
    let setup = parse_oql_file(
        r#"
        schema users {
          id: String,
          balance: Integer,
          profile: Optional(Object),
          items: List(Object)
        }
        users.insert { id: "u1", balance: 100, profile: { city: "Ibadan" }, items: [{ sku: "book", quantity: 1 }, { sku: "pen", quantity: 2 }] }
        users.insert { id: "u2", balance: 40, profile: null, items: [] }
        users.insert { id: "u3", balance: 0, items: [] }
        "#,
    )
    .unwrap();
    assert!(execute_oql_file(&setup, &executor)
        .unwrap()
        .errors
        .is_empty());

    let updated = executor
        .execute_query(
            &parse_oql(
                r#"
                users.where { .id == "u1" }.update {
                  profile.city: "Lagos",
                  profile.country: "NG",
                  balance += 25,
                  items: .items.select {
                    sku,
                    quantity: if .sku == "book" then .quantity + 1 else .quantity
                  }
                }
                "#,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(updated[0].get_field("balance"), Some(&Value::Integer(125)));
    assert_eq!(
        updated[0].get_field("profile"),
        Some(&Value::Object(owl_db::Fields::from_iter([
            ("city".into(), Value::String("Lagos".into())),
            ("country".into(), Value::String("NG".into())),
        ])))
    );
    let Value::List(items) = updated[0].get_field("items").unwrap() else {
        panic!("items must remain a list");
    };
    let Value::Object(book) = &items[0] else {
        panic!("item must remain an object");
    };
    assert_eq!(book.get("quantity"), Some(&Value::Integer(2)));

    let explicit_null_is_not_a_missing_object = executor.execute_query(
        &parse_oql(r#"users.where { .id == "u2" }.update { profile.city: "Lagos" }"#).unwrap(),
    );
    assert!(explicit_null_is_not_a_missing_object
        .unwrap_err()
        .to_string()
        .contains("not an object"));

    let created = executor
        .execute_query(
            &parse_oql(r#"users.where { .id == "u3" }.update { profile.city: "Lagos" }"#).unwrap(),
        )
        .unwrap();
    assert_eq!(
        created[0].get_field("profile"),
        Some(&Value::Object(owl_db::Fields::from_iter([(
            "city".into(),
            Value::String("Lagos".into())
        )])))
    );
}

#[test]
fn upsert_uses_one_declared_identity_key_for_insert_and_update() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("upsert.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    let file = parse_oql_file(
        r#"
        schema users {
          id: String,
          name: String,
          visits: Integer
        }
        initial_name = "Ada"
        users.upsert on id { id: "u1", name: initial_name, visits: 1 }
        users.upsert on id { id: "u1", name: "Ada Lovelace", visits: 2 }
        "#,
    )
    .unwrap();
    let result = execute_oql_file(&file, &executor).unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);

    let users = store.scan_documents("users").unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(
        users[0].get_field("name"),
        Some(&Value::String("Ada Lovelace".into()))
    );
    assert_eq!(users[0].get_field("visits"), Some(&Value::Integer(2)));

    let missing_key = parse_oql(r#"users.upsert on id { name: "Nobody", visits: 1 }"#)
        .unwrap_err()
        .to_string();
    assert!(missing_key.contains("requires exactly one"));
}

#[test]
fn having_evaluates_aggregate_expressions_over_each_group() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("having.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    for (id, status, amount) in [
        ("o1", "paid", Value::Integer(60)),
        ("o2", "paid", Value::Integer(70)),
        ("o3", "paid", Value::Null),
        ("o4", "pending", Value::Integer(200)),
    ] {
        store
            .put_document(&Document::with_id(
                id.into(),
                "orders".into(),
                owl_db::Fields::from_iter([
                    ("status".into(), Value::String(status.into())),
                    ("amount".into(), amount),
                ]),
            ))
            .unwrap();
    }

    let rows = executor
        .execute_query(
            &parse_oql(
                r#"
                orders
                  .group_by { .status }
                  .having { count() >= 3 && count(.amount) == 2 && sum(.amount) > 100 }
                  .select {
                    status,
                    row_count: count(),
                    amount_count: count(.amount),
                    total: sum(.amount),
                    average: avg(.amount),
                    minimum: min(.amount),
                    maximum: max(.amount),
                    average_again: sum(.amount) / count(.amount)
                  }
                "#,
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get_field("status"),
        Some(&Value::String("paid".into()))
    );
    assert_eq!(rows[0].get_field("row_count"), Some(&Value::Integer(3)));
    assert_eq!(rows[0].get_field("amount_count"), Some(&Value::Integer(2)));
    assert_eq!(rows[0].get_field("total"), Some(&Value::Integer(130)));
    assert_eq!(rows[0].get_field("average"), Some(&Value::Float(65.0)));
    assert_eq!(rows[0].get_field("minimum"), Some(&Value::Integer(60)));
    assert_eq!(rows[0].get_field("maximum"), Some(&Value::Integer(70)));
    assert_eq!(
        rows[0].get_field("average_again"),
        Some(&Value::Integer(65))
    );
}

#[test]
fn ungrouped_aggregate_calls_remain_composable_expressions() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(RustStore::new(temp.path().join("aggregates.owl"), 16 << 20).unwrap());
    let executor = QueryExecutor::new_with_real_clock(store.clone());
    for (id, amount) in [
        ("o1", Value::Integer(40)),
        ("o2", Value::Integer(60)),
        ("o3", Value::Null),
    ] {
        store
            .put_document(&Document::with_id(
                id.into(),
                "orders".into(),
                owl_db::Fields::from_iter([("amount".into(), amount)]),
            ))
            .unwrap();
    }

    let rows = executor
        .execute_query(
            &parse_oql(
                r#"
                orders.select {
                  rows: count(),
                  amounts: count(.amount),
                  average: sum(.amount) / count(.amount),
                  healthy: if avg(.amount) >= 50 then true else false,
                  summary: {
                    minimum: min(.amount),
                    maximum: max(.amount)
                  }
                }
                "#,
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_field("rows"), Some(&Value::Integer(3)));
    assert_eq!(rows[0].get_field("amounts"), Some(&Value::Integer(2)));
    assert_eq!(rows[0].get_field("average"), Some(&Value::Integer(50)));
    assert_eq!(rows[0].get_field("healthy"), Some(&Value::Boolean(true)));
    assert_eq!(
        rows[0].get_field("summary"),
        Some(&Value::Object(owl_db::Fields::from_iter([
            ("minimum".into(), Value::Integer(40)),
            ("maximum".into(), Value::Integer(60)),
        ])))
    );
}
