package owl_test

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"testing"
	"time"

	owl "github.com/TimiBolu/owl/drivers/go"
)

const token = "test-token"

// start runs an owl-server on a free port over an empty database and
// returns a client for it. The server binary is taken from OWL_SERVER_BIN,
// or from this repository's target directory.
func start(t testing.TB) *owl.Client {
	t.Helper()
	binary := os.Getenv("OWL_SERVER_BIN")
	if binary == "" {
		for _, profile := range []string{"release", "debug"} {
			candidate, _ := filepath.Abs(filepath.Join("..", "..", "target", profile, "owl-server"))
			if _, err := os.Stat(candidate); err == nil {
				binary = candidate
				break
			}
		}
	}
	if binary == "" {
		t.Skip("no owl-server binary: run `cargo build -p owl-server` or set OWL_SERVER_BIN")
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	address := listener.Addr().String()
	listener.Close()

	server := exec.Command(binary,
		"--data", filepath.Join(t.TempDir(), "db.owl"),
		"--listen", address,
		"--token", token,
		"--max-gib", "1",
		"--transaction-idle-seconds", "1")
	server.Stdout, server.Stderr = os.Stderr, os.Stderr
	if err := server.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = server.Process.Kill()
		_, _ = server.Process.Wait()
	})

	// Keep a connection per goroutine alive, as a service would: Go's
	// default keeps two per host and reconnects for the rest.
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.MaxIdleConnsPerHost = 64
	client := owl.New("http://"+address, owl.WithToken(token), owl.WithHTTPClient(&http.Client{Transport: transport}))
	deadline := time.Now().Add(15 * time.Second)
	for client.Ping(context.Background()) != nil {
		if time.Now().After(deadline) {
			t.Fatal("owl-server did not come up")
		}
		time.Sleep(20 * time.Millisecond)
	}
	return client
}

type Item struct {
	SKU string `json:"sku"`
	Qty int    `json:"qty"`
}

type Order struct {
	ID       string    `json:"id"`
	Customer string    `json:"customer"`
	Total    float64   `json:"total"`
	Stock    int64     `json:"stock"`
	Placed   time.Time `json:"placed"`
	Items    []Item    `json:"items"`
	Note     *string   `json:"note,omitempty"`
}

func shop(t *testing.T) (*owl.Client, context.Context) {
	t.Helper()
	db := start(t)
	ctx := context.Background()
	err := db.Script(ctx, `
		schema customers { id: String, name: String, email: String unique }
		schema orders {
		  id: String,
		  customer: Ref(customers),
		  total: Float,
		  stock: Integer = 0,
		  placed: DateTime,
		  items: List({ sku: String, qty: Integer }),
		  note: Optional(String)
		}
	`)
	if err != nil {
		t.Fatal(err)
	}
	ids, err := db.Insert(ctx, "customers", []map[string]any{
		{"id": "c1", "name": "Ada", "email": "ada@x.org"},
		{"id": "c2", "name": "Bo", "email": "bo@x.org"},
	})
	if err != nil || len(ids) != 2 {
		t.Fatalf("insert customers: %v %v", ids, err)
	}
	gift := "gift"
	ids, err = db.Insert(ctx, "orders", []Order{
		{ID: "o1", Customer: "c1", Total: 12.5, Stock: 10, Placed: time.Date(2026, 1, 5, 10, 0, 0, 0, time.UTC), Items: []Item{{"a", 2}}},
		{ID: "o2", Customer: "c2", Total: 40, Stock: 10, Placed: time.Date(2026, 2, 1, 0, 0, 0, 0, time.UTC), Items: []Item{}, Note: &gift},
	})
	if err != nil || fmt.Sprint(ids) != "[o1 o2]" {
		t.Fatalf("insert orders: %v %v", ids, err)
	}
	return db, ctx
}

func TestRowsAndStructs(t *testing.T) {
	db, ctx := shop(t)

	var orders []Order
	err := db.QueryInto(ctx, &orders, `orders.where { .total >= $least }.sort { .id.asc }`, owl.Params{"least": 10})
	if err != nil {
		t.Fatal(err)
	}
	if len(orders) != 2 || orders[0].Customer != "c1" || orders[0].Items[0] != (Item{"a", 2}) ||
		!orders[0].Placed.Equal(time.Date(2026, 1, 5, 10, 0, 0, 0, time.UTC)) ||
		orders[1].Note == nil || *orders[1].Note != "gift" || orders[0].Note != nil {
		t.Fatalf("%+v", orders)
	}

	rows, err := db.Query(ctx, `orders.sort { .id.asc }.select { id, who: .customer.name, stock, total, lines: .items.count() }`, nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(rows) != 2 || rows[0]["who"] != "Ada" || rows[0]["stock"] != int64(10) ||
		rows[0]["total"] != 12.5 || rows[1]["lines"] != int64(0) {
		t.Fatalf("%#v", rows)
	}

	rows, err = db.Query(ctx, `customers.where { matches(.email, $pattern) }.select { id }`, owl.Params{"pattern": "^bo@"})
	if err != nil || len(rows) != 1 || rows[0]["id"] != "c2" {
		t.Fatalf("%v %v", rows, err)
	}
}

func TestParametersAreNeverQueryText(t *testing.T) {
	db, ctx := shop(t)
	hostile := `x" } .delete //`
	rows, err := db.Query(ctx, `customers.where { .name == $name }.count()`, owl.Params{"name": hostile})
	if err != nil || rows[0]["count"] != int64(0) {
		t.Fatalf("%v %v", rows, err)
	}
	_, err = db.Query(ctx, `customers.where { .id == $id }.update { name: $name }`, owl.Params{"id": "c1", "name": hostile})
	if err != nil {
		t.Fatal(err)
	}
	rows, err = db.Query(ctx, `customers.sort { .id.asc }.select { name }`, nil)
	if err != nil || len(rows) != 2 || rows[0]["name"] != hostile {
		t.Fatalf("%v %v", rows, err)
	}
}

func TestFailuresCarryACode(t *testing.T) {
	db, ctx := shop(t)
	cases := []struct {
		name string
		run  func() error
		code string
	}{
		{"parse", func() error { _, err := db.Query(ctx, `orders.where {`, nil); return err }, "parse_error"},
		{"unknown field", func() error { _, err := db.Query(ctx, `orders.select { nope }`, nil); return err }, "rejected"},
		{"missing parameter", func() error { _, err := db.Query(ctx, `orders.where { .id == $id }`, nil); return err }, "invalid_request"},
		{"duplicate id", func() error {
			_, err := db.Insert(ctx, "customers", []map[string]any{{"id": "c1", "name": "Dup", "email": "d@x.org"}})
			return err
		}, "rejected"},
		{"dangling reference", func() error {
			_, err := db.Insert(ctx, "orders", []map[string]any{{"id": "o9", "customer": "nobody", "total": 1.0, "placed": "2026-01-01", "items": []any{}}})
			return err
		}, "rejected"},
		{"bad token", func() error {
			return owl.New(clientAddress(db), owl.WithToken("wrong")).Script(ctx, "schema x { id: String }")
		}, "unauthorized"},
	}
	for _, c := range cases {
		err := c.run()
		var failure *owl.Error
		if !errors.As(err, &failure) || failure.Code != c.code || failure.Message == "" {
			t.Errorf("%s: got %v, want code %s", c.name, err, c.code)
		}
		if owl.IsConflict(err) {
			t.Errorf("%s: reported as a conflict", c.name)
		}
	}
	// A batch with one bad document inserts none of it.
	_, err := db.Insert(ctx, "customers", []map[string]any{
		{"id": "c7", "name": "Fine", "email": "f@x.org"},
		{"id": "c8", "name": "Clash", "email": "ada@x.org"},
	})
	if err == nil {
		t.Fatal("a repeated unique email was accepted")
	}
	rows, _ := db.Query(ctx, `customers.count()`, nil)
	if rows[0]["count"] != int64(2) {
		t.Fatalf("%v", rows)
	}
}

func TestTransactionCommitsOrLeavesNothing(t *testing.T) {
	db, ctx := shop(t)
	count := func() int64 {
		rows, err := db.Query(ctx, `customers.count()`, nil)
		if err != nil {
			t.Fatal(err)
		}
		return rows[0]["count"].(int64)
	}

	err := db.Transaction(ctx, func(tx *owl.Tx) error {
		if _, err := tx.Insert(ctx, "customers", []map[string]any{{"id": "c3", "name": "Cy", "email": "cy@x.org"}}); err != nil {
			return err
		}
		rows, err := tx.Query(ctx, `customers.count()`, nil)
		if err != nil {
			return err
		}
		if rows[0]["count"] != int64(3) || count() != 2 {
			return fmt.Errorf("inside saw %v, outside saw %d", rows[0]["count"], count())
		}
		_, err = tx.Query(ctx, `orders.where { .id == "o1" }.update { total: 99.0 }`, nil)
		return err
	})
	if err != nil || count() != 3 {
		t.Fatalf("%v, %d customers", err, count())
	}

	failed := errors.New("changed my mind")
	err = db.Transaction(ctx, func(tx *owl.Tx) error {
		if _, err := tx.Query(ctx, `customers.where { .id == "c3" }.delete`, nil); err != nil {
			return err
		}
		return failed
	})
	if !errors.Is(err, failed) || count() != 3 {
		t.Fatalf("%v, %d customers", err, count())
	}

	// By hand: a committed transaction is over, and Rollback after it is
	// harmless, so it can be deferred.
	tx, err := db.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer tx.Rollback(ctx)
	if _, err := tx.Query(ctx, `customers.where { .id == "c3" }.update { name: "Cyd" }`, nil); err != nil {
		t.Fatal(err)
	}
	if err := tx.Commit(ctx); err != nil {
		t.Fatal(err)
	}
	if tx.Commit(ctx) == nil {
		t.Fatal("a second commit was accepted")
	}
}

func TestConflictIsReportedAndRetried(t *testing.T) {
	db, ctx := shop(t)
	first, _ := db.Begin(ctx)
	second, _ := db.Begin(ctx)
	for _, tx := range []*owl.Tx{first, second} {
		if _, err := tx.Query(ctx, `orders.where { .id == "o1" }.update { total: .total + 1 }`, nil); err != nil {
			t.Fatal(err)
		}
	}
	if err := first.Commit(ctx); err != nil {
		t.Fatal(err)
	}
	if err := second.Commit(ctx); !owl.IsConflict(err) {
		t.Fatalf("want a conflict, got %v", err)
	}

	// Transaction runs the work again when it loses.
	runs := 0
	err := db.Transaction(ctx, func(tx *owl.Tx) error {
		runs++
		var seen []Order
		if err := tx.QueryInto(ctx, &seen, `orders.where { .id == "o2" }`, nil); err != nil {
			return err
		}
		if runs == 1 {
			// Someone else gets in first.
			if _, err := db.Query(ctx, `orders.where { .id == "o2" }.update { total: 500.0 }`, nil); err != nil {
				return err
			}
		}
		_, err := tx.Query(ctx, `orders.where { .id == "o2" }.update { total: $total }`, owl.Params{"total": seen[0].Total + 1})
		return err
	})
	rows, _ := db.Query(ctx, `orders.where { .id == "o2" }.select { total }`, nil)
	if err != nil || runs != 2 || rows[0]["total"] != float64(501) {
		t.Fatalf("err %v, %d runs, %v", err, runs, rows)
	}

	// With one attempt allowed, the conflict comes back to the caller.
	once := owl.New(clientAddress(db), owl.WithToken(token), owl.WithTransactionAttempts(1))
	err = once.Transaction(ctx, func(tx *owl.Tx) error {
		if _, err := db.Query(ctx, `orders.where { .id == "o2" }.update { total: 1.0 }`, nil); err != nil {
			return err
		}
		_, err := tx.Query(ctx, `orders.where { .id == "o2" }.update { total: 2.0 }`, nil)
		return err
	})
	if !owl.IsConflict(err) {
		t.Fatalf("want a conflict, got %v", err)
	}
}

func TestManyGoroutinesLoseNothing(t *testing.T) {
	db, ctx := shop(t)
	const workers, each = 12, 10
	var wait sync.WaitGroup
	var mutex sync.Mutex
	taken := 0
	failures := make(chan error, workers)
	for worker := 0; worker < workers; worker++ {
		wait.Add(1)
		go func() {
			defer wait.Done()
			for round := 0; round < each; round++ {
				// One statement: take one from stock if any is left.
				rows, err := db.Query(ctx, `orders.where { .id == "o1" && .stock > 0 }.update { stock: .stock - 1 }`, nil)
				if err != nil {
					failures <- err
					return
				}
				mutex.Lock()
				taken += len(rows)
				mutex.Unlock()
				// A transfer: read two rows, write two rows.
				err = owl.New(clientAddress(db), owl.WithToken(token), owl.WithTransactionAttempts(1000)).
					Transaction(ctx, func(tx *owl.Tx) error {
						var both []Order
						if err := tx.QueryInto(ctx, &both, `orders.sort { .id.asc }`, nil); err != nil {
							return err
						}
						if _, err := tx.Query(ctx, `orders.where { .id == "o1" }.update { total: $t }`, owl.Params{"t": both[0].Total - 1}); err != nil {
							return err
						}
						_, err := tx.Query(ctx, `orders.where { .id == "o2" }.update { total: $t }`, owl.Params{"t": both[1].Total + 1})
						return err
					})
				if err != nil {
					failures <- err
					return
				}
			}
		}()
	}
	wait.Wait()
	close(failures)
	for err := range failures {
		t.Fatal(err)
	}
	var orders []Order
	if err := db.QueryInto(ctx, &orders, `orders.sort { .id.asc }`, nil); err != nil {
		t.Fatal(err)
	}
	moved := float64(workers * each)
	if taken != 10 || orders[0].Stock != 0 || orders[0].Total != 12.5-moved || orders[1].Total != 40+moved {
		t.Fatalf("taken %d, %+v", taken, orders)
	}
}

func TestAnAbandonedTransactionIsRolledBackByTheServer(t *testing.T) {
	db, ctx := shop(t)
	tx, err := db.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := tx.Query(ctx, `orders.where { .id == "o1" }.delete`, nil); err != nil {
		t.Fatal(err)
	}
	// The server was started with a one second idle limit.
	time.Sleep(1800 * time.Millisecond)
	err = tx.Commit(ctx)
	var failure *owl.Error
	if !errors.As(err, &failure) || failure.Code != "no_such_transaction" {
		t.Fatalf("want no_such_transaction, got %v", err)
	}
	rows, _ := db.Query(ctx, `orders.count()`, nil)
	if rows[0]["count"] != int64(2) {
		t.Fatalf("%v", rows)
	}
}

func benchShop(b *testing.B) (*owl.Client, context.Context) {
	b.Helper()
	db := start(b)
	ctx := context.Background()
	if err := db.Script(ctx, `schema items { id: String, name: String, stock: Integer, price: Float }`); err != nil {
		b.Fatal(err)
	}
	items := make([]map[string]any, 5000)
	for index := range items {
		items[index] = map[string]any{"id": fmt.Sprintf("i%05d", index), "name": fmt.Sprintf("item %05d", index), "stock": 1 << 40, "price": float64(index) / 2}
	}
	if _, err := db.Insert(ctx, "items", items); err != nil {
		b.Fatal(err)
	}
	return db, ctx
}

// One document by id, from as many goroutines as there are processors.
func BenchmarkPointRead(b *testing.B) {
	db, ctx := benchShop(b)
	b.ResetTimer()
	b.RunParallel(func(run *testing.PB) {
		for run.Next() {
			rows, err := db.Query(ctx, `items.where { .id == $id }.select { id, name, stock }`, owl.Params{"id": "i01234"})
			if err != nil || len(rows) != 1 {
				b.Fatal(rows, err)
			}
		}
	})
}

// Every goroutine decrements the same document: the worst case for
// conflicts, each one a durable commit.
func BenchmarkAtomicUpdateOneDocument(b *testing.B) {
	db, ctx := benchShop(b)
	b.ResetTimer()
	b.RunParallel(func(run *testing.PB) {
		for run.Next() {
			if _, err := db.Query(ctx, `items.where { .id == "i00001" }.update { stock: .stock - 1 }`, nil); err != nil {
				b.Fatal(err)
			}
		}
	})
}

// clientAddress recovers the address a test client was made for, so a
// test can make another client with different options.
func clientAddress(db *owl.Client) string {
	return owl.Address(db)
}
