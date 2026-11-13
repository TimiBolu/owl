const state = { events: [], selected: null, oql: "", loading: false };
const $ = (selector) => document.querySelector(selector);
const list = $("#event-list");
const status = $("#stream-status");

document.addEventListener("DOMContentLoaded", () => {
  bind();
  checkHealth();
  runInvestigation();
});

function bind() {
  $("#filter-form").addEventListener("submit", (event) => { event.preventDefault(); runInvestigation(); });
  $("#refresh").addEventListener("click", runInvestigation);
  $("#oql-toggle").addEventListener("click", toggleOql);
  $("#oql-code").addEventListener("input", (event) => { state.oql = event.target.value; });
  $("#copy-oql").addEventListener("click", async () => { await navigator.clipboard.writeText($("#oql-code").value); announce("OQL copied"); });
  $("#run-oql").addEventListener("click", runOql);
  document.querySelectorAll(".saved-view").forEach((button) => button.addEventListener("click", () => {
    document.querySelectorAll(".saved-view").forEach((item) => item.classList.remove("active"));
    button.classList.add("active");
    $("#level").value = button.dataset.level;
    runInvestigation();
  }));
  document.addEventListener("keydown", (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") { event.preventDefault(); toggleOql(); }
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter" && !$("#oql-panel").hidden) { event.preventDefault(); runOql(); }
    if (event.key === "Escape" && $("#detail").classList.contains("open")) { $("#detail").classList.remove("open"); list.querySelector(".selected")?.focus(); }
  });
}

async function checkHealth() {
  try {
    const response = await fetch("/api/v1/health");
    if (!response.ok) throw new Error();
    $("#connection-dot").classList.add("connected");
    $("#connection-status").textContent = "durable · local";
  } catch {
    $("#connection-status").textContent = "disconnected";
  }
}

async function runInvestigation() {
  if (state.loading) return;
  state.loading = true;
  list.setAttribute("aria-busy", "true");
  status.textContent = "Asking Owl for the latest evidence…";
  const form = new FormData($("#filter-form"));
  const params = new URLSearchParams({ application: form.get("application"), environment: form.get("environment"), limit: "200" });
  if (form.get("level")) params.set("level", form.get("level"));
  if (form.get("text")) params.set("text", form.get("text"));
  $("#scope-label").textContent = `${form.get("application")} / ${form.get("environment")}`;
  $("#stream-title").textContent = `${capitalize(form.get("environment"))} evidence`;
  try {
    const response = await fetch(`/api/v1/events?${params}`);
    const payload = await response.json();
    if (!response.ok) throw new Error(payload.error || "Query failed");
    state.events = payload.data;
    state.oql = payload.oql;
    $("#oql-code").value = payload.oql;
    $("#result-count").textContent = `${payload.data.length} event${payload.data.length === 1 ? "" : "s"} · ${payload.elapsed_ms.toFixed(2)} ms`;
    $("#access-path").textContent = payload.access_path;
    renderEvents();
    status.textContent = payload.data.length ? "" : "No evidence matched this investigation.";
  } catch (error) {
    list.replaceChildren();
    status.className = "stream-status error-state";
    status.textContent = error.message;
    announce(`Investigation failed: ${error.message}`);
  } finally {
    state.loading = false;
    list.setAttribute("aria-busy", "false");
  }
}

async function runOql() {
  const query = $("#oql-code").value.trim();
  if (!query) { announce("Enter an OQL query first"); return; }
  list.setAttribute("aria-busy", "true");
  status.textContent = "Executing OQL through Owl…";
  try {
    const response = await fetch("/api/v1/query", {method:"POST", headers:{"content-type":"application/json"}, body:JSON.stringify({query})});
    const payload = await response.json();
    if (!response.ok) throw new Error(payload.error || "OQL query failed");
    state.oql = query;
    $("#scope-label").textContent = "custom OQL";
    $("#stream-title").textContent = "OQL results";
    document.querySelectorAll(".saved-view").forEach((item) => item.classList.remove("active"));
    state.events = payload.data.map((document) => ({
      id: document.id,
      _schema: document.schema,
      application: document.fields.application || document.schema,
      environment: document.fields.environment || "",
      occurred_at: document.fields.occurred_at || document.fields.last_seen || document.updated_at,
      level: document.fields.level || document.fields.status || "result",
      message: document.fields.message || document.fields.title || `${document.schema}:${document.id}`,
      fingerprint: document.fields.fingerprint || null,
      trace_id: document.fields.trace_id || null,
      release: document.fields.release || document.fields.last_release || null,
    }));
    $("#result-count").textContent = `${payload.count} OQL result${payload.count === 1 ? "" : "s"} · ${payload.elapsed_ms.toFixed(2)} ms total · ${payload.execution_ms.toFixed(2)} ms execute · ${payload.materialization_ms.toFixed(2)} ms materialize · ${payload.serialization_ms.toFixed(2)} ms serialize`;
    $("#access-path").textContent = payload.access_path;
    $("#access-path").title = payload.plan || payload.access_path;
    renderEvents();
    status.textContent = payload.count ? "" : "OQL returned no documents.";
  } catch (error) {
    status.className = "stream-status error-state";
    status.textContent = error.message;
    announce(`OQL failed: ${error.message}`);
  } finally {
    list.setAttribute("aria-busy", "false");
  }
}

function renderEvents() {
  list.replaceChildren();
  status.className = "stream-status";
  if (!state.events.length) return;
  state.events.forEach((event) => {
    const item = document.createElement("li");
    item.className = "event-row";
    const button = document.createElement("button");
    button.className = "event-button";
    button.dataset.level = event.level;
    button.type = "button";
    button.setAttribute("aria-label", `${event.level} at ${formatTime(event.occurred_at)}: ${event.message}`);
    button.append(
      node("span", "event-time", formatTime(event.occurred_at)),
      node("span", "event-message", event.message),
      node("span", "event-level", event.level),
      node("span", "event-meta", `${event.release || "no release"}  ·  ${event.fingerprint ? short(event.fingerprint) : "ungrouped"}${event.trace_id ? "  ·  trace " + short(event.trace_id) : ""}`)
    );
    if (!event._schema || event._schema === "events") button.addEventListener("click", () => selectEvent(event.id, button));
    item.append(button);
    list.append(item);
  });
}

async function selectEvent(id, button) {
  list.querySelectorAll(".selected").forEach((row) => row.classList.remove("selected"));
  button.classList.add("selected");
  state.selected = id;
  $("#detail-empty").hidden = true;
  const content = $("#detail-content");
  content.hidden = false;
  content.replaceChildren(node("div", "detail-empty", "Loading occurrence…"));
  $("#detail").classList.add("open");
  try {
    const response = await fetch(`/api/v1/events/${encodeURIComponent(id)}`);
    const event = await response.json();
    if (!response.ok) throw new Error(event.error || "Occurrence unavailable");
    let issue = null;
    let trace = [];
    if (event.fields.fingerprint) {
      const issueResponse = await fetch(`/api/v1/issues/${encodeURIComponent(event.fields.fingerprint)}`);
      if (issueResponse.ok) issue = await issueResponse.json();
    }
    if (event.fields.trace_id) {
      const traceResponse = await fetch(`/api/v1/traces/${encodeURIComponent(event.fields.trace_id)}`);
      if (traceResponse.ok) trace = (await traceResponse.json()).data;
    }
    renderDetail(event, issue, trace);
  } catch (error) {
    content.replaceChildren(node("div", "error-state", error.message));
  }
}

function renderDetail(event, issue, trace) {
  const fields = event.fields;
  const content = $("#detail-content");
  content.replaceChildren();
  const shell = node("div", "detail-shell");
  const head = node("header", "detail-head");
  const top = node("div", "detail-head-top");
  const close = node("button", "close-detail", "×");
  close.type = "button";
  close.setAttribute("aria-label", "Close occurrence");
  close.addEventListener("click", closeDetail);
  top.append(node("span", "level-word", fields.level.toUpperCase()), node("time", "", new Date(fields.occurred_at).toLocaleString()), close);
  const title = node("h2", "", fields.message);
  const metadata = node("div", "metadata");
  [fields.application, fields.environment, fields.release || "no release", fields.exception_type || fields.kind].forEach((value) => metadata.append(node("span", "", value)));
  head.append(top, title, metadata);
  shell.append(head);
  if (fields.fingerprint) {
    const strip = node("section", "issue-strip");
    strip.append(node("strong", "", short(fields.fingerprint, 24)), node("span", "", issue ? `${issue.fields.occurrences} occurrences` : "issue"), node("small", "", issue ? `${issue.fields.status} · first seen ${new Date(issue.fields.first_seen).toLocaleString()}` : "Fingerprint-linked issue"));
    shell.append(strip);
  }
  shell.append(stackSection(fields.stack || []));
  if (fields.trace_id) shell.append(traceSection(fields.trace_id, trace, event.id));
  shell.append(jsonSection("Structured attributes", fields.attributes || {}));
  shell.append(jsonSection("Raw occurrence", event));
  content.append(shell);
}

function stackSection(stack) {
  const section = node("section", "detail-section");
  section.append(node("h3", "", `Execution spine · ${stack.length} frames`));
  const frames = node("ol", "stack");
  if (!stack.length) frames.append(node("li", "frame", "No stack frames were supplied."));
  stack.forEach((frame) => {
    const item = node("li", `frame${frame.in_app ? " in-app" : ""}`);
    const location = `${frame.file}${frame.line ? ":" + frame.line : ""}${frame.column ? ":" + frame.column : ""}`;
    const button = node("button", "", frame.function || "anonymous");
    button.type = "button";
    button.title = "Copy source location";
    button.addEventListener("click", async () => { await navigator.clipboard.writeText(location); announce(`${location} copied`); });
    item.append(button, node("small", "", location));
    frames.append(item);
  });
  section.append(frames);
  return section;
}

function traceSection(traceId, events, selectedId) {
  const section = node("section", "detail-section");
  section.append(node("h3", "", `Trace context · ${events.length} events`), node("code", "trace-link", traceId));
  const context = node("ol", "trace-events");
  events.slice(0, 8).forEach((event) => {
    const item = node("li", event.id === selectedId ? "current" : "");
    item.append(node("time", "", formatTime(event.occurred_at)), node("span", "", event.message));
    context.append(item);
  });
  section.append(context);
  return section;
}

function jsonSection(title, value) {
  const section = node("section", "detail-section");
  section.append(node("h3", "", title));
  const pre = node("pre", "json-view");
  pre.textContent = JSON.stringify(value, null, 2);
  section.append(pre);
  return section;
}

function toggleOql() {
  const panel = $("#oql-panel");
  const open = panel.hidden;
  panel.hidden = !open;
  $("#oql-toggle").setAttribute("aria-expanded", String(open));
  if (open) $("#oql-code").focus();
}

function closeDetail() {
  $("#detail").classList.remove("open");
  list.querySelector(".selected")?.focus();
}

function node(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}
function short(value, length = 12) { return value.length > length ? `${value.slice(0, length)}…` : value; }
function capitalize(value) { return value ? value.charAt(0).toUpperCase() + value.slice(1) : ""; }
function formatTime(value) { return new Date(value).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" }); }
function announce(message) { $("#announcer").textContent = message; }
