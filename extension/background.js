// Holds the one connection to a translation engine and shares it between the side panels and
// page translation. The engine is the desktop app's browser host (native messaging), or the
// in-browser engine (engine.js in an offscreen document, with wllama) when the app is not
// connected or the panel's settings ask for it. Both speak the host's protocol, described in
// crates/host/src/protocol.rs.
//
// - Panels connect here (port "panel"). Their translations start at once; a page translation
//   in progress is interrupted and its unfinished paragraphs are sent again afterwards.
// - Page translation runs in page.js (port "page"), which sends paragraphs; they go to the
//   host in small batches while nothing else is running.
// - The connection closes after IDLE_MS without work, which ends the host (or closes the
//   offscreen document) and frees the model's memory. Chrome keeps this service worker alive
//   while the host's connection is open; the in-browser engine sends `keepalive` messages.
// - The context menus and page.js show texts in the display language chosen in the panel.

importScripts("i18n.js");

const HOST = "com.github.tanakh.plamo_translate";
const IDLE_MS = 5 * 60 * 1000;
const BATCH_SEGMENTS = 10;
const BATCH_CHARS = 2000;
const MENU_SELECTION = "translate-selection";
const MENU_PAGE = "translate-page";
const MENUS = [
  { id: MENU_SELECTION, message: "menuTranslate", contexts: ["selection"] },
  { id: MENU_PAGE, message: "menuTranslatePage", contexts: ["page"] },
];

/**
 * The engine connection: { kind: "app" | "browser", port }. Both are null until the engine is
 * chosen and connected (see `hostReady`).
 */
let host = null;
/** Settles once the new connection's `configure` is sent; messages wait for it. */
let hostReady = Promise.resolve();
/** Whether the app's host answered, for automatic engine selection; asked again after it fails. */
let appAvailable;
/** Resolves with the in-browser engine's port once its document connects. */
let engineConnected = null;
/** Models the in-browser engine is downloading: the connection is busy meanwhile. */
const downloads = new Set();
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

// ---- Engines ----

/** The engine to use: the panel's choice, or on automatic the app if its host answers. */
async function chooseEngine() {
  const { engine } = await chrome.storage.local.get({ engine: "auto" }).catch(() => ({ engine: "auto" }));
  if (engine === "app" || engine === "browser") return engine;
  appAvailable ??= await probeApp();
  return appAvailable ? "app" : "browser";
}

/** Whether the app's host starts and answers: it is registered, and the app is where it was. */
function probeApp() {
  return new Promise((resolve) => {
    const port = chrome.runtime.connectNative(HOST);
    port.onMessage.addListener(() => {
      resolve(true);
      port.disconnect();
    });
    port.onDisconnect.addListener(() => resolve(false));
    port.postMessage({ type: "status" });
  });
}

/** Opens the in-browser engine's document and waits for it to connect. */
async function openEngine() {
  // Left over from a service worker that stopped: its model belongs to no connection.
  if ((await chrome.runtime.getContexts({ contextTypes: ["OFFSCREEN_DOCUMENT"] })).length) {
    await chrome.offscreen.closeDocument();
  }
  const connected = new Promise((resolve) => (engineConnected = resolve));
  await chrome.offscreen.createDocument({
    url: "engine.html",
    reasons: ["WORKERS"],
    justification: "Runs the translation model with WebAssembly and WebGPU when the desktop app is not connected.",
  });
  return connected;
}

function connectHost() {
  const connection = { kind: null, port: null };
  host = connection;
  hostReady = (async () => {
    const kind = await chooseEngine();
    const port = kind === "browser" ? await openEngine() : chrome.runtime.connectNative(HOST);
    connection.kind = kind;
    connection.port = port;
    if (host !== connection) {
      closeConnection(connection);
      return;
    }
    port.onMessage.addListener((message) => onHostMessage(connection, message));
    port.onDisconnect.addListener(() => {
      const error = chrome.runtime.lastError?.message ?? "";
      if (connection.kind === "app") appAvailable = undefined;
      if (host !== connection) return;
      host = null;
      clearTimeout(idleTimer);
      downloads.clear();
      status = { type: "status", state: "disconnected", engine: kind, message: error };
      broadcast(status);
      failAll({ code: "disconnected", message: error });
    });
    // The engine loads a model only after this, so the default one is not loaded in vain. Each
    // engine has its own choices: their models and devices differ.
    const key = kind === "browser" ? "browserChoices" : "choices";
    const { [key]: choices } = await chrome.storage.local.get({ [key]: {} }).catch(() => ({ [key]: {} }));
    if (host === connection) {
      port.postMessage({ type: "configure", model: choices.model || undefined, device: choices.device || undefined });
    }
  })().catch((e) => {
    if (host !== connection) return;
    host = null;
    status = { type: "status", state: "disconnected", engine: connection.kind, message: String(e?.message ?? e) };
    broadcast(status);
    failAll({ code: "disconnected", message: status.message });
  });
}

/** Ends a connection from this side, which fires no onDisconnect. */
function closeConnection(connection) {
  connection.port?.disconnect();
  if (connection.kind === "browser") chrome.offscreen.closeDocument().catch(() => {});
}

/** Sends `message` after `configure`, connecting first if needed; messages keep their order. */
async function sendHost(message) {
  if (!host) connectHost();
  const connection = host;
  await hostReady;
  // Dropped if the connection ended meanwhile.
  if (host !== connection || !connection.port) return;
  connection.port.postMessage(message);
  touch();
}

function touch() {
  clearTimeout(idleTimer);
  if (host) idleTimer = setTimeout(releaseIfIdle, IDLE_MS);
}

function releaseIfIdle() {
  if (jobs.size || downloads.size || [...pages.values()].some((p) => p.pending.length)) return touch();
  release("idle");
}

function release(reason) {
  if (!host) return;
  closeConnection(host);
  host = null;
  downloads.clear();
  clearTimeout(idleTimer);
  // Closing our end fires no onDisconnect; end the jobs here.
  for (const job of jobs.values()) {
    if (job.kind === "panel") {
      job.port.postMessage({ type: "done", id: job.id, tokens: 0, cut: 0, stopped: true, cancelled: true });
    }
  }
  jobs.clear();
  active = null;
  for (const page of pages.values()) {
    page.pending = [];
    page.port.postMessage({ type: "error", code: "released" });
  }
  status = { type: "status", state: "released", reason };
  broadcast(status);
  reportPages();
}

function onHostMessage(connection, message) {
  if (host !== connection) return;
  // Only keeps this service worker alive: not a use of the model.
  if (message.type === "keepalive") return;
  touch();
  switch (message.type) {
    case "status":
      status = { ...message, engine: connection.kind };
      broadcast(status);
      return;
    case "options":
      options = { ...message, engine: connection.kind };
      broadcast(options);
      return;
    case "download":
      if (message.state === "running") downloads.add(message.model);
      else downloads.delete(message.model);
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
  reportPages();
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
  const undone = job.items.filter((item) => !item.done);
  if (undone.length && !message.cancelled) {
    // Cut short otherwise; the same batch would stop again. Resume in the page tries again.
    page.pending = [];
    page.port.postMessage({ type: "error", code: "failed", message: "the translation stopped early" });
    return;
  }
  // Paragraphs not translated because the batch was cancelled (a panel translation
  // interrupted it, or the model changed) go back to the front of the queue.
  page.pending.unshift(...undone);
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
  reportPages();
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

/** The tabs whose page translation runs: paragraphs waiting, or a batch at the engine. */
function runningPages() {
  return [...pages]
    .filter(([tabId, page]) => page.pending.length || [...jobs.values()].some((j) => j.kind === "page" && j.tabId === tabId && !j.cancelled))
    .map(([tabId]) => tabId);
}

let reportedPages = "";
/** Tells the panels which tabs are translating, when that changes (their page button stops it). */
function reportPages(port = null) {
  const running = runningPages();
  const key = running.join(",");
  if (port) port.postMessage({ type: "pages", running });
  else if (key !== reportedPages) broadcast({ type: "pages", running });
  reportedPages = key;
}

// ---- Panels ----

function onPanelMessage(port, message) {
  switch (message.type) {
    case "hello":
      port.postMessage(status);
      if (options) port.postMessage(options);
      reportPages(port);
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
    // The in-browser engine's models, connecting to it if it was released (or not started yet).
    case "download":
    case "delete":
      (async () => {
        const kind = host ? host.kind : await chooseEngine();
        if (kind === "browser" || kind === null) sendHost(message);
      })();
      break;
    case "set_engine":
      // Saved, then the current engine is released and the new one asked for its options.
      chrome.storage.local.set({ engine: message.engine }).then(() => {
        appAvailable = undefined;
        release("engine");
        sendHost({ type: "options" });
      });
      break;
    case "release":
      release("requested");
      break;
    case "stop_page": {
      const page = pages.get(message.tabId);
      stopPage(message.tabId);
      page?.port.postMessage({ type: "stopped", reason: "panel" });
      break;
    }
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
  // page.js asks for the display language's messages ("ui_messages") when it starts.
  await chrome.scripting.executeScript({ target: { tabId }, files: ["i18n.js", "page.js"] });
}

function onPageMessage(port, message) {
  const tabId = port.sender?.tab?.id;
  if (tabId == null) return;
  switch (message.type) {
    case "segments": {
      // A page translation that starts (not more paragraphs of a running one) stops those of
      // other tabs: the user has moved on. They can resume from their own page.
      if (message.start) {
        for (const [other, page] of pages) {
          if (other === tabId) continue;
          const running = page.pending.length || [...jobs.values()].some((j) => j.kind === "page" && j.tabId === other);
          if (!running) continue;
          stopPage(other);
          page.port.postMessage({ type: "stopped" });
        }
      }
      const page = pages.get(tabId) ?? { port, pending: [] };
      page.port = port;
      page.target = message.target;
      page.style = message.style;
      page.pending.push(...message.items);
      pages.set(tabId, page);
      schedule();
      reportPages();
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
  reportPages();
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
  } else if (port.name === "engine") {
    // The in-browser engine's document, opened by openEngine.
    if (engineConnected) {
      engineConnected(port);
      engineConnected = null;
    } else {
      port.disconnect();
    }
  } else if (port.name === "page") {
    port.onMessage.addListener((message) => onPageMessage(port, message));
    port.onDisconnect.addListener(() => {
      const tabId = port.sender?.tab?.id;
      const page = pages.get(tabId);
      if (page?.port !== port) return;
      stopPage(tabId);
      pages.delete(tabId);
      reportPages();
    });
  }
});

// The display language, read again when the panel changes it.
let languageReady = loadUiLanguage();
chrome.storage.local.onChanged.addListener((changes) => {
  if (!changes.uiLanguage) return;
  languageReady = loadUiLanguage().then(() => {
    for (const { id, message } of MENUS) chrome.contextMenus.update(id, { title: t(message) }).catch(() => {});
  });
});

chrome.runtime.onInstalled.addListener(() => {
  languageReady.then(() => {
    for (const { id, message, contexts } of MENUS) chrome.contextMenus.create({ id, title: t(message), contexts });
  });
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type !== "ui_messages") return false;
  languageReady.then(() => sendResponse(uiMessages ?? null));
  return true;
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
