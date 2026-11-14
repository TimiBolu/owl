# Owl driver for Go

A client for [owl-server](../../apps/owl-server). Standard library only.

```bash
go get github.com/TimiBolu/owl/drivers/go
```

```go
import owl "github.com/TimiBolu/owl/drivers/go"

db := owl.New("http://127.0.0.1:7878", owl.WithToken(os.Getenv("OWL_TOKEN")))

// Declare the schema once.
err := db.Script(ctx, `
    schema customers { id: String, name: String }
    schema orders { id: String, customer: Ref(customers), total: Float, stock: Integer = 0 }
`)

// Insert structs or maps. A reference is the id it points at.
ids, err := db.Insert(ctx, "orders", []Order{{ID: "o1", Customer: "c1", Total: 12.5}})

// Query into structs, matched by their json tags. Values go in parameters.
var orders []Order
err = db.QueryInto(ctx, &orders,
    `orders.where { .customer == $who && .total >= $least }.sort { .id.asc }`,
    owl.Params{"who": "c1", "least": 10})

// Or into rows: whole numbers are int64, other numbers float64.
rows, err := db.Query(ctx, `orders.select { id, who: .customer.name }`, nil)
```

## Writes that must not be lost

One statement is atomic by itself, so most updates need no transaction:

```go
// Takes one from stock only if there is one. len(rows) says whether it did.
rows, err := db.Query(ctx,
    `orders.where { .id == $id && .stock > 0 }.update { stock: .stock - 1 }`,
    owl.Params{"id": "o1"})
```

To make several statements one step, hand the work to `Transaction`. It
commits what the function did, rolls back if the function returns an error,
and runs the function again if another commit got there first:

```go
err := db.Transaction(ctx, func(tx *owl.Tx) error {
    var accounts []Account
    if err := tx.QueryInto(ctx, &accounts, `accounts.where { .id == $a || .id == $b }.sort { .id.asc }`, owl.Params{"a": from, "b": to}); err != nil {
        return err
    }
    if _, err := tx.Query(ctx, `accounts.where { .id == $id }.update { balance: $v }`, owl.Params{"id": from, "v": accounts[0].Balance - amount}); err != nil {
        return err
    }
    _, err := tx.Query(ctx, `accounts.where { .id == $id }.update { balance: $v }`, owl.Params{"id": to, "v": accounts[1].Balance + amount})
    return err
})
```

Because the function may run more than once, it must read what it needs
afresh each time and must not send an email or charge a card inside it.

`Begin`, `Commit` and `Rollback` are there for when you need to hold the
transaction yourself. `owl.IsConflict(err)` tells a lost commit from any
other failure.

## Failures

A failure reported by the server is an `*owl.Error` with the HTTP `Status`,
a `Code` (`conflict`, `rejected`, `parse_error`, `invalid_request`,
`no_such_transaction`, `unauthorized`, `internal`) and a `Message`.

## Testing

The tests start a real server. Build it first:

```bash
cargo build -p owl-server
```

```bash
go test ./...
```
