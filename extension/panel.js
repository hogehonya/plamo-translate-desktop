// Side panel: sends text to the desktop app's browser host over native messaging and shows the
// translation as it streams in. The host protocol is described in crates/host/src/protocol.rs.

const HOST = "com.github.tanakh.plamo_translate";

// Target languages offered here; the host accepts any of the model's 53 codes.
const TARGETS = ["auto", "ja", "en", "zh-hans", "zh-hant", "ko", "fr", "de", "es", "it", "pt", "ru", "vi", "th", "id"];
const STYLES = ["", "polite", "plain"];

const $ = (id) => document.getElementById(id);
const t = (key, ...args) => chrome.i18n.getMessage(key, args) || key;

let port = null;
let nextId = 1;
let currentId = null;
let outputNode = document.createTextNode("");
let status = null;

// ---- UI setup ----

for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
for (const el of document.querySelectorAll("[data-i18n-placeholder]")) {
  el.placeholder = t(el.dataset.i18nPlaceholder);
}
$("output").append(outputNode);

for (const code of TARGETS) $("target").add(new Option(t(`lang_${code.replace("-", "_")}`), code));
for (const style of STYLES) $("style").add(new Option(t(`style_${style || "default"}`), style));

chrome.storage.local.get({ target: "auto", style: "" }).then(({ target, style }) => {
  $("target").value = target;
  $("style").value = style;
  updateStyleVisibility();
});
$("target").addEventListener("change", () => {
  chrome.storage.local.set({ target: $("target").value });
  updateStyleVisibility();
});
$("style").addEventListener("change", () => chrome.storage.local.set({ style: $("style").value }));

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
$("copy").addEventListener("click", async () => {
  await navigator.clipboard.writeText(outputNode.data);
  $("copy").textContent = t("copied");
  setTimeout(() => ($("copy").textContent = t("copy")), 1200);
});

// Text selected in a page arrives through session storage (see background.js).
chrome.storage.session.get("request").then(({ request }) => request && takeRequest(request));
chrome.storage.session.onChanged.addListener((changes) => {
  if (changes.request?.newValue) takeRequest(changes.request.newValue);
});

function takeRequest(request) {
  chrome.storage.session.remove("request");
  $("source").value = request.text;
  translate(request.text);
}

// ---- Native messaging ----

function connect() {
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener(onMessage);
  port.onDisconnect.addListener(() => {
    const error = chrome.runtime.lastError?.message ?? "";
    port = null;
    if (currentId !== null) finish();
    // The host could not be started: it is not registered, or the app was moved.
    showNotice(/not found|forbidden|exited/i.test(error) ? t("noticeSetup") : t("noticeDisconnected", error));
    setStatus(t("statusDisconnected"), true);
  });
}

function send(message) {
  if (!port) connect();
  port.postMessage(message);
}

function translate(text) {
  if (!text.trim()) return;
  hideNotice();
  currentId = nextId++;
  outputNode.data = "";
  $("progress").textContent = "";
  $("copy").disabled = true;
  setTranslating(true);
  send({
    type: "translate",
    id: currentId,
    text,
    target: $("target").value,
    style: $("style").value || undefined,
  });
}

function onMessage(message) {
  switch (message.type) {
    case "status":
      status = message;
      renderStatus();
      break;
    case "started":
      if (message.id === currentId) $("progress").textContent = t("translatingInto", t(`lang_${message.target.replace("-", "_")}`));
      break;
    case "progress":
      if (message.id === currentId && message.count > 1) {
        $("progress").textContent = `${message.index + 1} / ${message.count}`;
      }
      break;
    case "text":
      if (message.id === currentId) outputNode.appendData(message.text);
      break;
    case "rewind":
      // Lengths are in UTF-16 code units, like JavaScript strings.
      if (message.id === currentId) outputNode.data = outputNode.data.slice(0, message.length);
      break;
    case "done":
      if (message.id === currentId) {
        $("progress").textContent = message.stopped
          ? t("stopped")
          : message.cut > 0
            ? t("doneCut", String(message.cut))
            : t("done", String(message.tokens));
        finish();
      }
      break;
    case "error":
      if (message.id == null || message.id === currentId) {
        showNotice(errorText(message));
        finish();
      }
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
    default:
      return t("errorFailed", message.message ?? message.code);
  }
}

// ---- Rendering ----

function renderStatus() {
  if (!status) return;
  const model = [status.model?.replace(/^plamo-3-translate-/, ""), status.device].filter(Boolean).join(" · ");
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
    default:
      setStatus(status.message ?? t("statusError"), true);
  }
}

function setStatus(text, isError = false) {
  $("status").textContent = text;
  $("status").classList.toggle("error", isError);
}

function setTranslating(translating) {
  const button = $("translate");
  button.textContent = translating ? t("stop") : t("translate");
  button.classList.toggle("primary", !translating);
  button.classList.toggle("danger", translating);
}

function showNotice(text) {
  $("notice").textContent = text;
  $("notice").hidden = false;
}

function hideNotice() {
  $("notice").hidden = true;
}

// Connect right away to show the model's state.
send({ type: "status" });
