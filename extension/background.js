// Holds the one connection to a translation engine and shares it between the side panels and
// page translation. The engine is the desktop app's browser host (native messaging), or the
// in-browser engine (engine.js in an offscreen document, with wllama) when the app is not
// connected or the panel's settings ask for it. Both speak the host's protocol, described in
// crates/host/src/protocol.rs.
//
// - Panels connect here (port "panel"). Their translations start at once; a page translation
//   in progress is interrupted and its unfinished paragraphs are sent again afterwards.
// - Page translation runs in page.js (port "page"), which sends paragraphs; they go to the
//   host in small batches while no panel translation runs. The app's host runs one batch at a
//   time; the in-browser engine runs several side by side (its status's `slots`).
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
/**
 * Whether the app's host answers (a promise), for automatic engine selection; asked again after
 * it fails.
 */
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
 * { id, kind: "panel", port }, { id, kind: "measure", port } (the in-browser engine measuring
 * its devices), or { id, kind: "page", tabId, items, cancelled, counted }.
 * A job superseded by a newer one stays here until the host reports it stopped.
 */
const jobs = new Map();
/** The jobs the host runs, not superseded: a panel translation or measurement, or page batches. */
const active = new Set();
/**
 * Page translations by tab: { port, target, style, pending: [{ sid, text }], busySince,
 * busyTokens }. The last two count the tokens since its batches started, for its speed.
 */
const pages = new Map();

// ---- Engines ----

/** The panel's engine setting: `auto`, `app`, or `browser`. */
async function engineSetting() {
  const { engine } = await chrome.storage.local.get({ engine: "auto" }).catch(() => ({ engine: "auto" }));
  return engine;
}

/** The engine to use: the panel's choice, or on automatic the app if its host answers. */
async function chooseEngine() {
  const engine = await engineSetting();
  if (engine === "app" || engine === "browser") return engine;
  appAvailable ??= probeApp();
  return (await appAvailable) ? "app" : "browser";
}

/** Tells the panels (or `port`) which engine the automatic setting uses, when it is the setting. */
async function reportEngine(port = null) {
  const setting = await engineSetting();
  const auto = setting === "auto" ? (host?.kind ?? (await chooseEngine())) : null;
  const message = { type: "engine", setting, auto };
  if (port) port.postMessage(message);
  else broadcast(message);
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
    reportEngine();
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
      port.postMessage({
        type: "configure",
        model: choices.model || undefined,
        device: choices.device || undefined,
        threads: choices.threads || undefined,
      });
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
    } else if (job.kind === "measure") {
      job.port.postMessage({ type: "measured", error: { code: "released" } });
    }
  }
  jobs.clear();
  active.clear();
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
      // The first one tells how many batches the engine runs at once.
      schedule();
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
    active.delete(job);
  }
  if (job.kind === "panel") {
    job.port.postMessage(message);
  } else if (job.kind === "measure") {
    if (finished) job.port.postMessage({ type: "measured", error: message.type === "error" ? message : null });
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
    // The in-browser engine tells each segment's tokens; the app's host only the batch's.
    if (message.tokens > 0 && !job.cancelled) {
      job.counted += message.tokens;
      countTokens(page, message.tokens);
    }
    return;
  }
  // The batch starts once the model is loaded: the speed leaves the loading out.
  if (message.type === "started" && page) page.busySince ??= performance.now();
  if (message.type !== "done" && message.type !== "error") return;
  if (!page) return;
  if (message.type === "done" && !job.cancelled) countTokens(page, message.tokens - job.counted);
  // The speed counts from the start of the page's batches running together.
  if (![...jobs.values()].some((j) => j.kind === "page" && j.tabId === job.tabId)) {
    page.busySince = null;
    page.busyTokens = 0;
  }
  if (job.cancelled) return;
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

/** Adds generated tokens to a page's count, and tells it its speed (prompts included). */
function countTokens(page, tokens) {
  if (!page || !(tokens > 0) || page.busySince == null) return;
  page.busyTokens += tokens;
  const seconds = (performance.now() - page.busySince) / 1000;
  if (seconds > 0) page.port.postMessage({ type: "speed", tokensPerSecond: page.busyTokens / seconds });
}

function failAll(error) {
  for (const job of jobs.values()) {
    if (job.kind === "panel") job.port.postMessage({ type: "error", id: job.id, ...error });
    else if (job.kind === "measure") job.port.postMessage({ type: "measured", error });
  }
  jobs.clear();
  active.clear();
  for (const page of pages.values()) {
    page.pending = [];
    page.port.postMessage({ type: "error", ...error });
  }
  reportPages();
}

/** How many page batches the host runs at once: one until the in-browser engine tells. */
const batchSlots = () => (host?.kind === "browser" && status.engine === "browser" && status.slots > 1 ? status.slots : 1);

/** Starts page batches while the host has room for them and no panel translation (or measurement) runs. */
function schedule() {
  if ([...active].some((job) => job.kind !== "page")) return;
  while (active.size < batchSlots()) {
    const entry = [...pages].find(([, page]) => page.pending.length);
    if (!entry) return;
    const [tabId, page] = entry;
    const items = [];
    let chars = 0;
    while (page.pending.length && items.length < BATCH_SEGMENTS && (chars < BATCH_CHARS || !items.length)) {
      const item = page.pending.shift();
      chars += item.text.length;
      items.push(item);
    }
    const job = { id: nextId++, kind: "page", tabId, items, cancelled: false, counted: 0 };
    jobs.set(job.id, job);
    active.add(job);
    sendHost({
      type: "translate_segments",
      id: job.id,
      segments: items.map((item) => item.text),
      target: page.target,
      style: page.style || undefined,
    });
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
      reportEngine(port);
      if (host) sendHost({ type: "options" });
      break;
    case "translate": {
      // Starts at once: the host stops whatever runs and reports it done (see onBatchMessage).
      const job = { id: nextId++, kind: "panel", port };
      jobs.set(job.id, job);
      active.clear();
      active.add(job);
      port.postMessage({ type: "accepted", id: job.id });
      sendHost({ type: "translate", id: job.id, text: message.text, target: message.target, style: message.style });
      break;
    }
    case "cancel":
      cancelPanel(port);
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
        reportEngine();
      });
      break;
    case "measure":
      // The in-browser engine measures the GPU and the CPU again; page batches wait for it.
      (async () => {
        // While the connection starts (as after the panel's `configure`), its kind is not known yet.
        if ((host?.kind ?? (await chooseEngine())) !== "browser") {
          port.postMessage({ type: "measured", error: { code: "failed", message: "not the in-browser engine" } });
          return;
        }
        const job = { id: nextId++, kind: "measure", port };
        jobs.set(job.id, job);
        active.clear();
        active.add(job);
        sendHost({ type: "measure", id: job.id });
        reportPages();
      })();
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
      const page = pages.get(tabId) ?? { port, pending: [], busySince: null, busyTokens: 0 };
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
    if (job.kind !== "page" || job.tabId !== tabId) continue;
    job.cancelled = true;
    // The app's host stops its one job, whatever the id; the in-browser engine this one.
    if (active.has(job)) sendHost({ type: "cancel", id: job.id });
  }
  reportPages();
}

/** Stops the panel's translation if it runs. */
function cancelPanel(port) {
  const job = [...active].find((j) => j.kind === "panel" && j.port === port);
  if (job) sendHost({ type: "cancel", id: job.id });
}

// ---- Wiring ----

chrome.runtime.onConnect.addListener((port) => {
  if (port.name === "panel") {
    panels.add(port);
    port.onMessage.addListener((message) => onPanelMessage(port, message));
    port.onDisconnect.addListener(() => {
      panels.delete(port);
      cancelPanel(port);
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
