// Holds the one connection to the desktop app's browser host (native messaging) and shares it
// between the side panels and page translation. The host protocol is described in
// crates/host/src/protocol.rs.
//
// - Panels connect here (port "panel"). Their translations start at once; a page translation
//   in progress is interrupted and its unfinished paragraphs are sent again afterwards.
// - Page translation runs in page.js (port "page"), which sends paragraphs; they go to the
//   host in small batches while nothing else is running.
// - The connection closes after IDLE_MS without work, which ends the host and frees the
//   model's memory. Chrome keeps this service worker alive while the connection is open.

const HOST = "com.github.tanakh.plamo_translate";
const IDLE_MS = 5 * 60 * 1000;
const BATCH_SEGMENTS = 10;
const BATCH_CHARS = 2000;
const MENU_SELECTION = "translate-selection";
const MENU_PAGE = "translate-page";

let host = null;
let idleTimer = null;
let status = { type: "status", state: "released" };
let options = null;
let nextId = 1;

/** Open panel ports. */
const panels = new Set();
/**
 * Jobs sent to the host whose `done` (or `error`) has not arrived, by id:
 * { id, kind: "panel", port } or { id, kind: "page", tabId, items, cancelled }.
 * A job superseded by a newer one stays here until the host reports it stopped.
 */
const jobs = new Map();
/** The newest job, which the host is running. */
let active = null;
/** Page translations by tab: { port, target, style, pending: [{ sid, text }] }. */
const pages = new Map();

// ---- Native host ----

async function connectHost() {
  host = chrome.runtime.connectNative(HOST);
  host.onMessage.addListener(onHostMessage);
  host.onDisconnect.addListener(() => {
    const error = chrome.runtime.lastError?.message ?? "";
    host = null;
    clearTimeout(idleTimer);
    status = { type: "status", state: "disconnected", message: error };
    broadcast(status);
    failAll({ code: "disconnected", message: error });
  });
  // The host loads a model only after this, so the app's model is not loaded in vain.
  const { choices } = await chrome.storage.local.get({ choices: {} });
  host?.postMessage({ type: "configure", model: choices.model || undefined, device: choices.device || undefined });
}

async function sendHost(message) {
  if (!host) await connectHost();
  host?.postMessage(message);
  touch();
}

function touch() {
  clearTimeout(idleTimer);
  if (host) idleTimer = setTimeout(releaseIfIdle, IDLE_MS);
}

function releaseIfIdle() {
  if (jobs.size || [...pages.values()].some((p) => p.pending.length)) return touch();
  release("idle");
}

function release(reason) {
  if (!host) return;
  host.disconnect();
  host = null;
  clearTimeout(idleTimer);
  // Closing our end fires no onDisconnect; end the jobs here.
  for (const job of jobs.values()) {
    if (job.kind === "panel") job.port.postMessage({ type: "done", id: job.id, tokens: 0, cut: 0, stopped: true });
  }
  jobs.clear();
  active = null;
  for (const page of pages.values()) {
    page.pending = [];
    page.port.postMessage({ type: "error", code: "released" });
  }
  status = { type: "status", state: "released", reason };
  broadcast(status);
}

function onHostMessage(message) {
  touch();
  switch (message.type) {
    case "status":
      status = message;
      broadcast(message);
      return;
    case "options":
      options = message;
      broadcast(message);
      return;
  }
  const job = jobs.get(message.id);
  if (!job) return;
  const finished = message.type === "done" || message.type === "error";
  if (finished) {
    jobs.delete(job.id);
    if (active === job) active = null;
  }
  if (job.kind === "panel") {
    job.port.postMessage(message);
  } else {
    onBatchMessage(job, message);
  }
  if (finished) schedule();
}

function onBatchMessage(job, message) {
  const page = pages.get(job.tabId);
  if (message.type === "segment") {
    const item = job.items[message.index];
    if (item) {
      item.done = true;
      page?.port.postMessage({ type: "segment", sid: item.sid, text: message.text });
    }
    return;
  }
  if (message.type !== "done" && message.type !== "error") return;
  if (!page || job.cancelled) return;
  if (message.type === "error") {
    page.pending = [];
    page.port.postMessage({ type: "error", code: message.code, message: message.message });
    return;
  }
  // Paragraphs not translated because a panel translation interrupted the batch go back to
  // the front of the queue.
  page.pending.unshift(...job.items.filter((item) => !item.done));
  reportPage(page);
}

function failAll(error) {
  for (const job of jobs.values()) {
    if (job.kind === "panel") job.port.postMessage({ type: "error", id: job.id, ...error });
  }
  jobs.clear();
  active = null;
  for (const page of pages.values()) {
    page.pending = [];
    page.port.postMessage({ type: "error", ...error });
  }
}

/** Starts the next page batch when the host is free. */
function schedule() {
  if (active) return;
  for (const [tabId, page] of pages) {
    if (!page.pending.length) continue;
    const items = [];
    let chars = 0;
    while (page.pending.length && items.length < BATCH_SEGMENTS && (chars < BATCH_CHARS || !items.length)) {
      const item = page.pending.shift();
      chars += item.text.length;
      items.push(item);
    }
    active = { id: nextId++, kind: "page", tabId, items, cancelled: false };
    jobs.set(active.id, active);
    sendHost({
      type: "translate_segments",
      id: active.id,
      segments: items.map((item) => item.text),
      target: page.target,
      style: page.style || undefined,
    });
    return;
  }
}

/** Tells a page when all its paragraphs are translated. */
function reportPage(page) {
  const busy = [...jobs.values()].some((job) => job.kind === "page" && pages.get(job.tabId) === page);
  if (!busy && !page.pending.length) page.port.postMessage({ type: "idle" });
}

function broadcast(message) {
  for (const port of panels) port.postMessage(message);
}

// ---- Panels ----

function onPanelMessage(port, message) {
  switch (message.type) {
    case "hello":
      port.postMessage(status);
      if (options) port.postMessage(options);
      if (host) sendHost({ type: "options" });
      break;
    case "translate": {
      // Starts at once: the host stops whatever runs and reports it done (see onBatchMessage).
      active = { id: nextId++, kind: "panel", port };
      jobs.set(active.id, active);
      port.postMessage({ type: "accepted", id: active.id });
      sendHost({ type: "translate", id: active.id, text: message.text, target: message.target, style: message.style });
      break;
    }
    case "cancel":
      if (active?.kind === "panel" && active.port === port) sendHost({ type: "cancel" });
      break;
    case "configure":
    case "options":
    case "status":
      sendHost(message);
      break;
    case "release":
      release("requested");
      break;
    case "translate_page":
      translatePage(message.tabId).then(
        () => port.postMessage({ type: "page_started" }),
        (e) => port.postMessage({ type: "page_failed", message: String(e?.message ?? e) }),
      );
      break;
  }
}

// ---- Page translation ----

async function translatePage(tabId) {
  await chrome.scripting.executeScript({ target: { tabId }, files: ["page.js"] });
}

function onPageMessage(port, message) {
  const tabId = port.sender?.tab?.id;
  if (tabId == null) return;
  switch (message.type) {
    case "segments": {
      const page = pages.get(tabId) ?? { port, pending: [] };
      page.port = port;
      page.target = message.target;
      page.style = message.style;
      page.pending.push(...message.items);
      pages.set(tabId, page);
      schedule();
      break;
    }
    case "cancel":
      stopPage(tabId);
      break;
    case "prioritize": {
      // Paragraphs the user scrolled to go first, in the order given.
      const page = pages.get(tabId);
      if (!page) break;
      const order = new Map(message.sids.map((sid, i) => [sid, i]));
      const first = page.pending.filter((item) => order.has(item.sid));
      first.sort((a, b) => order.get(a.sid) - order.get(b.sid));
      page.pending = [...first, ...page.pending.filter((item) => !order.has(item.sid))];
      break;
    }
  }
}

function stopPage(tabId) {
  const page = pages.get(tabId);
  if (page) page.pending = [];
  for (const job of jobs.values()) {
    if (job.kind === "page" && job.tabId === tabId) job.cancelled = true;
  }
  if (active?.kind === "page" && active.tabId === tabId) sendHost({ type: "cancel" });
}

// ---- Wiring ----

chrome.runtime.onConnect.addListener((port) => {
  if (port.name === "panel") {
    panels.add(port);
    port.onMessage.addListener((message) => onPanelMessage(port, message));
    port.onDisconnect.addListener(() => {
      panels.delete(port);
      if (active?.kind === "panel" && active.port === port) sendHost({ type: "cancel" });
    });
  } else if (port.name === "page") {
    port.onMessage.addListener((message) => onPageMessage(port, message));
    port.onDisconnect.addListener(() => {
      const tabId = port.sender?.tab?.id;
      const page = pages.get(tabId);
      if (page?.port !== port) return;
      stopPage(tabId);
      pages.delete(tabId);
    });
  }
});

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: MENU_SELECTION,
    title: chrome.i18n.getMessage("menuTranslate"),
    contexts: ["selection"],
  });
  chrome.contextMenus.create({
    id: MENU_PAGE,
    title: chrome.i18n.getMessage("menuTranslatePage"),
    contexts: ["page"],
  });
});

chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch(() => {});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === MENU_PAGE && tab?.id != null) {
    translatePage(tab.id).catch(() => {});
    return;
  }
  if (info.menuItemId !== MENU_SELECTION || !info.selectionText) return;
  // Must be called while handling the click, before anything asynchronous.
  chrome.sidePanel.open({ windowId: tab.windowId }).catch(() => {});
  // The panel picks this up when it opens, or right away if it is open already.
  chrome.storage.session.set({ request: { text: info.selectionText, at: Date.now() } });
});
