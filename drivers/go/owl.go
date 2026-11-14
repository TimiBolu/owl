// Package owl is a client for owl-server.
//
// A Client is safe for use by many goroutines and holds no connection
// state of its own: every call is one HTTP request.
//
//	db := owl.New("http://127.0.0.1:7878", owl.WithToken(token))
//
//	var orders []Order
//	err := db.QueryInto(ctx, &orders,
//		`orders.where { .customer == $who && .total >= $least }`,
//		owl.Params{"who": "c1", "least": 100})
//
// Values belong in parameters. A parameter is never read as OQL, whatever
// it contains.
//
// A statement run on the client is its own commit and is atomic by itself:
// `update { stock: .stock - 1 }` from two callers takes two off. To make
// several statements one step, use Transaction, which runs a function and
// commits what it did, running it again if another commit got there first.
package owl

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
)

// Params are the values of a query's $name parameters.
type Params map[string]any

// Row is one result row: field names to values. A whole number is an
// int64, any other number a float64, a timestamp RFC 3339 text, and a
// reference the id of the document it points at.
type Row map[string]any

// Error is a failure reported by the server.
type Error struct {
	// Status is the HTTP status of the response.
	Status int
	// Code says what kind of failure it is: "conflict", "rejected",
	// "parse_error", "invalid_request", "no_such_transaction",
	// "unauthorized", "not_found" or "internal".
	Code string
	// Message is the server's account of it.
	Message string
}

func (e *Error) Error() string {
	return fmt.Sprintf("owl: %s (%s)", e.Message, e.Code)
}

// IsConflict reports whether err is a commit refused because another
// commit changed a document the transaction wrote. Nothing was applied,
// and running the transaction again from the start is the remedy.
func IsConflict(err error) bool {
	var failure *Error
	return errors.As(err, &failure) && failure.Code == "conflict"
}

// Client talks to one owl-server.
type Client struct {
	base     string
	token    string
	http     *http.Client
	attempts int
}

// Option adjusts a Client.
type Option func(*Client)

// WithToken sends token as the bearer token on every request.
func WithToken(token string) Option {
	return func(c *Client) { c.token = token }
}

// WithHTTPClient uses client for requests, for its timeouts, transport or
// connection pool.
func WithHTTPClient(client *http.Client) Option {
	return func(c *Client) { c.http = client }
}

// WithTransactionAttempts sets how many times Transaction runs its
// function before giving up on a conflict. The default is 16.
func WithTransactionAttempts(attempts int) Option {
	return func(c *Client) {
		if attempts > 0 {
			c.attempts = attempts
		}
	}
}

// New returns a client for the server at address, such as
// "http://127.0.0.1:7878".
func New(address string, options ...Option) *Client {
	client := &Client{
		base:     strings.TrimRight(address, "/"),
		http:     http.DefaultClient,
		attempts: 16,
	}
	for _, option := range options {
		option(client)
	}
	return client
}

// Ping reports whether the server is reachable and answering.
func (c *Client) Ping(ctx context.Context) error {
	return c.call(ctx, http.MethodGet, "/v1/health", nil, nil)
}

// Query runs one OQL statement and returns its rows.
func (c *Client) Query(ctx context.Context, query string, params Params) ([]Row, error) {
	return c.query(ctx, 0, query, params)
}

// QueryInto runs one OQL statement and decodes its rows into dest, which
// must point to a slice. Fields are matched to struct fields by their
// `json` tags.
func (c *Client) QueryInto(ctx context.Context, dest any, query string, params Params) error {
	return c.queryInto(ctx, 0, dest, query, params)
}

// Insert adds documents to a collection in one commit: all of them or, if
// any is refused, none. documents is a slice of structs or maps, encoded
// as JSON. A reference is given as the id it points at and a timestamp as
// RFC 3339 text or a time.Time. It returns the ids of the documents
// inserted.
func (c *Client) Insert(ctx context.Context, collection string, documents any) ([]string, error) {
	return c.insert(ctx, 0, collection, documents)
}

// Script runs an OQL script: schema declarations, functions and
// statements, in order. It is how a schema is created or changed.
func (c *Client) Script(ctx context.Context, source string) error {
	return c.call(ctx, http.MethodPost, "/v1/script", map[string]any{"source": source}, nil)
}

// Tx is an open transaction. What it writes is seen by nobody else until
// it commits. It is not safe for use by several goroutines at once.
type Tx struct {
	client *Client
	id     uint64
	done   bool
}

// Begin opens a transaction. Prefer Transaction, which commits, rolls back
// and retries for you. A transaction left untouched for longer than the
// server's idle time is rolled back by the server.
func (c *Client) Begin(ctx context.Context) (*Tx, error) {
	var opened struct {
		Transaction uint64 `json:"transaction"`
	}
	if err := c.call(ctx, http.MethodPost, "/v1/transactions", struct{}{}, &opened); err != nil {
		return nil, err
	}
	return &Tx{client: c, id: opened.Transaction}, nil
}

// Query runs one statement inside the transaction.
func (t *Tx) Query(ctx context.Context, query string, params Params) ([]Row, error) {
	return t.client.query(ctx, t.id, query, params)
}

// QueryInto runs one statement inside the transaction and decodes its rows
// into dest.
func (t *Tx) QueryInto(ctx context.Context, dest any, query string, params Params) error {
	return t.client.queryInto(ctx, t.id, dest, query, params)
}

// Insert adds documents inside the transaction.
func (t *Tx) Insert(ctx context.Context, collection string, documents any) ([]string, error) {
	return t.client.insert(ctx, t.id, collection, documents)
}

// Commit applies everything the transaction wrote, or nothing. Either way
// the transaction is over. If IsConflict holds for the error, run the work
// again in a new transaction.
func (t *Tx) Commit(ctx context.Context) error {
	if t.done {
		return errors.New("owl: the transaction is already over")
	}
	t.done = true
	return t.client.call(ctx, http.MethodPost, fmt.Sprintf("/v1/transactions/%d/commit", t.id), struct{}{}, nil)
}

// Rollback abandons the transaction. Calling it after Commit or a second
// time does nothing, so it can be deferred.
func (t *Tx) Rollback(ctx context.Context) error {
	if t.done {
		return nil
	}
	t.done = true
	return t.client.call(ctx, http.MethodPost, fmt.Sprintf("/v1/transactions/%d/rollback", t.id), struct{}{}, nil)
}

// Transaction runs work in a transaction and commits it.
//
// If work returns an error the transaction is rolled back and the error
// returned. If the commit is refused because another commit changed a
// document this one wrote, work is run again in a new transaction, so it
// must read what it needs afresh each time and must not do anything
// outside the database that cannot be repeated. After the configured
// number of attempts the conflict is returned.
func (c *Client) Transaction(ctx context.Context, work func(tx *Tx) error) error {
	var conflict error
	for attempt := 0; attempt < c.attempts; attempt++ {
		tx, err := c.Begin(ctx)
		if err != nil {
			return err
		}
		if err := work(tx); err != nil {
			// The work's error is the one worth reporting.
			_ = tx.Rollback(context.WithoutCancel(ctx))
			return err
		}
		err = tx.Commit(ctx)
		if err == nil {
			return nil
		}
		if !IsConflict(err) {
			return err
		}
		conflict = err
		if ctx.Err() != nil {
			return ctx.Err()
		}
	}
	return conflict
}

func (c *Client) request(transaction uint64, fields map[string]any) map[string]any {
	if transaction != 0 {
		fields["transaction"] = transaction
	}
	return fields
}

func (c *Client) query(ctx context.Context, transaction uint64, query string, params Params) ([]Row, error) {
	var answer struct {
		Rows []json.RawMessage `json:"rows"`
	}
	if err := c.runQuery(ctx, transaction, query, params, &answer); err != nil {
		return nil, err
	}
	rows := make([]Row, len(answer.Rows))
	for index, raw := range answer.Rows {
		decoder := json.NewDecoder(bytes.NewReader(raw))
		decoder.UseNumber()
		var fields map[string]any
		if err := decoder.Decode(&fields); err != nil {
			return nil, fmt.Errorf("owl: decode row: %w", err)
		}
		rows[index] = Row(plain(fields).(map[string]any))
	}
	return rows, nil
}

func (c *Client) queryInto(ctx context.Context, transaction uint64, dest any, query string, params Params) error {
	var answer struct {
		Rows json.RawMessage `json:"rows"`
	}
	if err := c.runQuery(ctx, transaction, query, params, &answer); err != nil {
		return err
	}
	if err := json.Unmarshal(answer.Rows, dest); err != nil {
		return fmt.Errorf("owl: decode rows: %w", err)
	}
	return nil
}

func (c *Client) runQuery(ctx context.Context, transaction uint64, query string, params Params, answer any) error {
	fields := map[string]any{"query": query}
	if len(params) > 0 {
		fields["params"] = params
	}
	return c.call(ctx, http.MethodPost, "/v1/query", c.request(transaction, fields), answer)
}

func (c *Client) insert(ctx context.Context, transaction uint64, collection string, documents any) ([]string, error) {
	var answer struct {
		IDs []string `json:"ids"`
	}
	fields := map[string]any{"collection": collection, "documents": documents}
	if err := c.call(ctx, http.MethodPost, "/v1/insert", c.request(transaction, fields), &answer); err != nil {
		return nil, err
	}
	return answer.IDs, nil
}

// plain turns decoded numbers into int64 where they are whole and fit, and
// float64 otherwise, at any depth.
func plain(value any) any {
	switch value := value.(type) {
	case json.Number:
		if integer, err := value.Int64(); err == nil {
			return integer
		}
		float, _ := value.Float64()
		return float
	case map[string]any:
		for name, field := range value {
			value[name] = plain(field)
		}
		return value
	case []any:
		for index, item := range value {
			value[index] = plain(item)
		}
		return value
	default:
		return value
	}
}

func (c *Client) call(ctx context.Context, method, path string, body, answer any) error {
	var payload io.Reader
	if body != nil {
		encoded, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("owl: encode request: %w", err)
		}
		payload = bytes.NewReader(encoded)
	}
	request, err := http.NewRequestWithContext(ctx, method, c.base+path, payload)
	if err != nil {
		return fmt.Errorf("owl: %w", err)
	}
	if body != nil {
		request.Header.Set("Content-Type", "application/json")
	}
	if c.token != "" {
		request.Header.Set("Authorization", "Bearer "+c.token)
	}
	response, err := c.http.Do(request)
	if err != nil {
		return fmt.Errorf("owl: %w", err)
	}
	defer response.Body.Close()
	content, err := io.ReadAll(response.Body)
	if err != nil {
		return fmt.Errorf("owl: read response: %w", err)
	}
	if response.StatusCode >= 300 {
		var failure struct {
			Error struct {
				Code    string `json:"code"`
				Message string `json:"message"`
			} `json:"error"`
		}
		if json.Unmarshal(content, &failure) != nil || failure.Error.Code == "" {
			// Not one of the server's own failures: a proxy, or a request
			// the server could not read.
			failure.Error.Code = "invalid_request"
			if response.StatusCode >= 500 {
				failure.Error.Code = "internal"
			}
			failure.Error.Message = strings.TrimSpace(string(content))
			if failure.Error.Message == "" {
				failure.Error.Message = response.Status
			}
		}
		return &Error{Status: response.StatusCode, Code: failure.Error.Code, Message: failure.Error.Message}
	}
	if answer == nil {
		return nil
	}
	if err := json.Unmarshal(content, answer); err != nil {
		return fmt.Errorf("owl: decode response: %w", err)
	}
	return nil
}
