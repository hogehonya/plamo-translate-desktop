// Side panel: translates text through the background (background.js), which holds the
// connection to the desktop app's browser host, and shows the translation as it streams in.
// It also starts page translation (page.js) and holds the extension's settings.

// Target languages, as in the app: Japanese ⇄ English, the model's 53 languages, and easy
// Japanese (experimental). Names come from the app's locale file (lang_* messages).
const TARGETS = [
  "auto", "ja", "en", "zh-hans", "zh-hant", "ko", "ar", "az", "be", "bg", "bn", "ca", "ceb", "de",
  "es", "et", "eu", "fa", "fr", "gu", "he", "hi", "hr", "hu", "hy", "id", "it", "jv", "ka", "kk",
  "km", "lt", "lv", "ml", "mr", "ms", "my", "ne", "nl", "pa", "pl", "ru", "si", "sk", "sl", "sr",
  "sw", "ta", "te", "th", "tl", "ur", "uz", "vi", "ja(easy)",
];
const STYLES = ["", "polite", "plain"];
const PAGE_MODES = ["replace", "below"];

const $ = (id) => document.getElementById(id);
const t = (key, ...args) => chrome.i18n.getMessage(key, args) || key;
const langName = (code) => t(`lang_${code.replaceAll("-", "_").replace("(easy)", "_easy")}`);

let port = null;
// The model and device chosen here ("" follows the app); the background sends them whenever
// it starts the host.
let choices = { model: "", device: "" };
/** The translation shown: true while waiting for the background to accept it, then its id. */
let currentId = null;
let outputNode = document.createTextNode("");

// ---- UI setup ----

for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
for (const el of document.querySelectorAll("[data-i18n-placeholder]")) {
  el.placeholder = t(el.dataset.i18nPlaceholder);
}
for (const el of document.querySelectorAll("[data-i18n-title]")) el.title = t(el.dataset.i18nTitle);
$("output").append(outputNode);

for (const code of TARGETS) $("target").add(new Option(langName(code), code));
for (const style of STYLES) $("style").add(new Option(t(`style_${style || "default"}`), style));
for (const mode of PAGE_MODES) $("pageMode").add(new Option(t(`pageMode_${mode}`), mode));

chrome.storage.local.get({ target: "auto", style: "", pageMode: "replace" }).then((saved) => {
  $("target").value = saved.target;
  $("style").value = saved.style;
  $("pageMode").value = saved.pageMode;
  updateStyleVisibility();
});
$("target").addEventListener("change", () => {
  chrome.storage.local.set({ target: $("target").value });
  updateStyleVisibility();
});
$("style").addEventListener("change", () => chrome.storage.local.set({ style: $("style").value }));
$("pageMode").addEventListener("change", () => chrome.storage.local.set({ pageMode: $("pageMode").value }));

for (const id of ["model", "device"]) {
  $(id).addEventListener("change", () => {
    choices[id] = $(id).value;
    chrome.storage.local.set({ choices });
    send({ type: "configure", model: choices.model || undefined, device: choices.device || undefined });
  });
}

function updateStyleVisibility() {
  // Style applies to Japanese only; "auto" may translate into Japanese.
  const target = $("target").value;
  $("style").hidden = !(target === "ja" || target === "auto");
}

$("translate").addEventListener("click", () => {
  if (currentId !== null) {
    send({ type: "cancel" });
  } else {
    translate($("source").value);
  }
});
$("source").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    translate($("source").value);
  }
});
$("release").addEventListener("click", () => send({ type: "release" }));
$("translatePage").addEventListener("click", translatePage);

async function translatePage() {
  hideNotice();
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (tab?.id != null) send({ type: "translate_page", tabId: tab.id });
}
$("copy").addEventListener("click", async () => {
  await navigator.clipboard.writeText(outputNode.data);
  $("copy").textContent = t("copied");
  setTimeout(() => ($("copy").textContent = t("copy")), 1200);
});

// The saved choices are needed before the host is first contacted.
const ready = chrome.storage.local.get({ choices }).then((saved) => {
  choices = { ...choices, ...saved.choices };
});

// Text selected in a page arrives through session storage (see background.js).
chrome.storage.session.get("request").then(({ request }) => request && ready.then(() => takeRequest(request)));
chrome.storage.session.onChanged.addListener((changes) => {
  const request = changes.request?.newValue;
  if (request) ready.then(() => takeRequest(request));
});

function takeRequest(request) {
  chrome.storage.session.remove("request");
  $("source").value = request.text;
  translate(request.text);
}

// ---- Talking to the background ----

function connect() {
  port = chrome.runtime.connect({ name: "panel" });
  port.onMessage.addListener(onMessage);
  // The background stops when idle; connect again on the next message.
  port.onDisconnect.addListener(() => {
    port = null;
    if (currentId !== null) finish();
  });
  port.postMessage({ type: "hello" });
}

function send(message) {
  if (!port) connect();
  port.postMessage(message);
}

function translate(text) {
  if (!text.trim()) return;
  hideNotice();
  currentId = true;
  outputNode.data = "";
  $("progress").textContent = "";
  $("copy").disabled = true;
  setTranslating(true);
  send({ type: "translate", text, target: $("target").value, style: $("style").value || undefined });
}

function onMessage(message) {
  switch (message.type) {
    case "status":
      renderStatus(message);
      return;
    case "options":
      renderOptions(message);
      return;
    case "accepted":
      if (currentId === true) currentId = message.id;
      return;
    case "page_failed":
      // Without the activeTab grant (the context menu or the toolbar button gives it), the
      // page can be reached only with permission for its site.
      showNotice(t("pageNotAllowed"), t("allowAllSites"), async () => {
        if (await chrome.permissions.request({ origins: ["<all_urls>"] })) translatePage();
      });
      return;
    case "page_started":
      return;
  }
  if (message.id !== currentId) return;
  switch (message.type) {
    case "started":
      $("progress").textContent = t("translatingInto", langName(message.target));
      break;
    case "progress":
      if (message.count > 1) $("progress").textContent = `${message.index + 1} / ${message.count}`;
      break;
    case "text":
      outputNode.appendData(message.text);
      break;
    case "rewind":
      // Lengths are in UTF-16 code units, like JavaScript strings.
      outputNode.data = outputNode.data.slice(0, message.length);
      break;
    case "done":
      $("progress").textContent = message.stopped
        ? t("stopped")
        : message.cut > 0
          ? t("doneCut", String(message.cut))
          : t("done", String(message.tokens));
      finish();
      break;
    case "error":
      showNotice(errorText(message));
      finish();
      break;
  }
}

function finish() {
  currentId = null;
  setTranslating(false);
  $("copy").disabled = outputNode.data.length === 0;
}

function errorText(message) {
  switch (message.code) {
    case "no_model":
      return t("errorNoModel");
    case "model_missing":
      return t("errorModelMissing");
    case "disconnected":
      return setupNeeded(message.message) ? t("noticeSetup") : t("noticeDisconnected", message.message ?? "");
    default:
      return t("errorFailed", message.message ?? message.code);
  }
}

/** Whether the host could not be started: not registered, or the app was moved. */
function setupNeeded(error) {
  return /not found|forbidden|exited/i.test(error ?? "");
}

// ---- Rendering ----

function renderOptions(options) {
  const gb = (bytes) => `${(bytes / 1e9).toFixed(2)} GB`;
  const model = $("model");
  model.replaceChildren(new Option(t("followApp", options.app_model ?? t("none")), ""));
  for (const m of options.models) model.add(new Option(`${m.label} (${gb(m.bytes)})`, m.id));
  model.value = options.model ?? "";

  const deviceName = (setting) => (setting === "auto" ? t("deviceAuto") : setting === "cpu" ? "CPU" : setting);
  const device = $("device");
  device.replaceChildren(
    new Option(t("followApp", deviceName(options.app_device)), ""),
    new Option(t("deviceAuto"), "auto"),
    new Option("CPU", "cpu"),
  );
  for (const d of options.devices) device.add(new Option(`${d.name} — ${d.description}`, d.name));
  device.value = options.device ?? "";
}

function renderStatus(status) {
  const model = [status.model, status.device].filter(Boolean).join(" · ");
  $("release").disabled = status.state === "released" || status.state === "disconnected";
  switch (status.state) {
    case "ready":
      setStatus(model);
      break;
    case "loading":
    case "starting":
      setStatus(t("statusLoading", model));
      break;
    case "no_model":
      setStatus(t("statusNoModel"), true);
      break;
    case "released":
      setStatus(status.reason === "idle" ? t("statusReleasedIdle") : t("statusReleased"), false, t("statusReleasedDetail"));
      break;
    case "disconnected":
      setStatus(t("statusDisconnected"), true);
      if (status.message) showNotice(setupNeeded(status.message) ? t("noticeSetup") : t("noticeDisconnected", status.message));
      break;
    default:
      setStatus(status.message ?? t("statusError"), true);
  }
}

function setStatus(text, isError = false, detail = "") {
  $("status").textContent = text;
  $("status").title = detail;
  $("status").classList.toggle("error", isError);
}

function setTranslating(translating) {
  const button = $("translate");
  button.textContent = translating ? t("stop") : t("translate");
  button.classList.toggle("primary", !translating);
  button.classList.toggle("danger", translating);
}

/** Shows `text`, with a button running `action` if `label` is given. */
function showNotice(text, label, action) {
  $("noticeText").textContent = text;
  const button = $("noticeAction");
  button.hidden = !label;
  button.textContent = label ?? "";
  button.onclick = action ?? null;
  $("notice").hidden = false;
}

function hideNotice() {
  $("notice").hidden = true;
}

// Connect right away, with the saved choices, so the model loads while the user reads.
ready.then(() => send({ type: "options" }));
