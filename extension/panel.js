// Side panel: translates text through the background (background.js), which holds the
// connection to the engine (the desktop app's browser host, or the in-browser engine), and shows
// the translation as it streams in.
// It also starts page translation (page.js) and holds the extension's settings.
// Texts come from i18n.js (`t`), in the display language chosen here or the browser's.

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
const ENGINES = ["auto", "app", "browser"];

const $ = (id) => document.getElementById(id);
const langName = (code) => t(`lang_${code.replaceAll("-", "_").replace("(easy)", "_easy")}`);

let port = null;
/** The engine in use, from its status and options: `app` or `browser`. */
let engine = "app";
// The model and device chosen here for each engine, by storage key ("" is the default: the
// app's choice, or the first model downloaded); the background sends them whenever it starts
// the engine.
const choices = { choices: { model: "", device: "" }, browserChoices: { model: "", device: "" } };
const choiceKey = () => (engine === "browser" ? "browserChoices" : "choices");
/** The in-browser engine's downloads, by model: its latest `download` message. */
const downloads = {};
let licenseAccepted = false;
/** The model to download once the license is accepted. */
let pendingDownload = null;
/** The translation shown: true while waiting for the background to accept it, then its id. */
let currentId = null;
let outputNode = document.createTextNode("");
// What the status line, the progress line, and the notice show, kept to show it again when the
// display language changes. Texts are functions giving them in the current language.
let shownStatus = null;
let shownOptions = null;
let progress = () => "";
let notice = null;

$("output").append(outputNode);

// ---- Settings ----

$("target").addEventListener("change", () => {
  chrome.storage.local.set({ target: $("target").value });
  updateStyleVisibility();
});
$("style").addEventListener("change", () => chrome.storage.local.set({ style: $("style").value }));
$("pageMode").addEventListener("change", () => chrome.storage.local.set({ pageMode: $("pageMode").value }));
$("uiLanguage").addEventListener("change", () => chrome.storage.local.set({ uiLanguage: $("uiLanguage").value }));
$("engine").addEventListener("change", () => {
  shownOptions = null;
  send({ type: "set_engine", engine: $("engine").value });
});
// Applied here as in panels of other windows (and in the background, for its menus).
chrome.storage.local.onChanged.addListener(async (changes) => {
  if (changes.engine) $("engine").value = changes.engine.newValue ?? "auto";
  if (!changes.uiLanguage) return;
  const uiLanguage = await loadUiLanguage();
  applyLanguage();
  $("uiLanguage").value = uiLanguage;
});

for (const id of ["model", "device"]) {
  $(id).addEventListener("change", () => {
    const key = choiceKey();
    choices[key][id] = $(id).value;
    chrome.storage.local.set({ [key]: choices[key] });
    send({ type: "configure", model: choices[key].model || undefined, device: choices[key].device || undefined });
  });
}

$("licenseLink").addEventListener("click", (e) => {
  e.preventDefault();
  chrome.tabs.create({ url: t("licenseUrl") });
});
$("licenseAccept").addEventListener("click", () => {
  licenseAccepted = true;
  chrome.storage.local.set({ licenseAccepted });
  if (pendingDownload) send({ type: "download", model: pendingDownload });
  pendingDownload = null;
  renderBrowserModels();
});

function updateStyleVisibility() {
  // Style applies to Japanese only; "auto" may translate into Japanese.
  const target = $("target").value;
  $("style").hidden = !(target === "ja" || target === "auto");
}

/** Shows every text in the display language: at startup and when the language changes. */
function applyLanguage() {
  document.documentElement.lang = uiLocale();
  for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  for (const el of document.querySelectorAll("[data-i18n-placeholder]")) {
    el.placeholder = t(el.dataset.i18nPlaceholder);
  }
  for (const el of document.querySelectorAll("[data-i18n-title]")) el.title = t(el.dataset.i18nTitle);

  fill($("target"), TARGETS, langName);
  fill($("style"), STYLES, (style) => t(`style_${style || "default"}`));
  fill($("pageMode"), PAGE_MODES, (mode) => t(`pageMode_${mode}`));
  fill($("engine"), ENGINES, (name) => t(`engine${name[0].toUpperCase()}${name.slice(1)}`));
  const browser = UI_LANGUAGES[chrome.i18n.getMessage("locale")] ?? "";
  fill($("uiLanguage"), ["auto", ...Object.keys(UI_LANGUAGES)], (lang) =>
    lang === "auto" ? t("uiLanguageAuto", browser) : UI_LANGUAGES[lang],
  );
  if (shownOptions) {
    // Keeps the choices made since the host's options arrived.
    const model = $("model").value;
    const device = $("device").value;
    renderOptions(shownOptions);
    $("model").value = model;
    $("device").value = device;
  }
  if (shownStatus) renderStatus(shownStatus);
  renderEngineTexts();
  setTranslating(currentId !== null);
  $("progress").textContent = progress();
  if (notice) renderNotice();
}

/** Fills `select` with `values`, labelled by `label`, keeping its selection. */
function fill(select, values, label) {
  const selected = select.value;
  select.replaceChildren(...values.map((value) => new Option(label(value), value)));
  if (values.includes(selected)) select.value = selected;
}

// ---- Actions ----

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

// The display language comes before any text is shown, and the saved choices before the host
// is first contacted.
const ready = Promise.all([
  loadUiLanguage(),
  chrome.storage.local.get({ target: "auto", style: "", pageMode: "replace", engine: "auto", licenseAccepted: false, ...choices }),
]).then(([uiLanguage, saved]) => {
  applyLanguage();
  $("uiLanguage").value = uiLanguage;
  $("target").value = saved.target;
  $("style").value = saved.style;
  $("pageMode").value = saved.pageMode;
  $("engine").value = saved.engine;
  updateStyleVisibility();
  for (const key of Object.keys(choices)) Object.assign(choices[key], saved[key]);
  licenseAccepted = saved.licenseAccepted;
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
  setProgress(() => "");
  $("copy").disabled = true;
  setTranslating(true);
  send({ type: "translate", text, target: $("target").value, style: $("style").value || undefined });
}

function onMessage(message) {
  switch (message.type) {
    case "status":
      renderStatus(message);
      if (message.state === "disconnected" && message.message) {
        showNotice(() =>
          message.engine === "browser"
            ? t("noticeEngineStopped", message.message)
            : setupNeeded(message.message)
              ? t("noticeSetup")
              : t("noticeDisconnected", message.message),
        );
      }
      return;
    case "options":
      renderOptions(message);
      return;
    case "download":
      downloads[message.model] = message;
      renderBrowserModels();
      return;
    case "accepted":
      if (currentId === true) currentId = message.id;
      return;
    case "page_failed":
      pageFailed(message.message);
      return;
    case "page_started":
      return;
  }
  if (message.id !== currentId) return;
  switch (message.type) {
    case "started":
      setProgress(() => t("translatingInto", langName(message.target)));
      break;
    case "progress":
      if (message.count > 1) setProgress(() => `${message.index + 1} / ${message.count}`);
      break;
    case "text":
      outputNode.appendData(message.text);
      break;
    case "rewind":
      // Lengths are in UTF-16 code units, like JavaScript strings.
      outputNode.data = outputNode.data.slice(0, message.length);
      break;
    case "done":
      setProgress(() =>
        message.stopped
          ? t("stopped")
          : message.cut > 0
            ? t("doneCut", String(message.cut))
            : t("done", String(message.tokens)),
      );
      finish();
      break;
    case "error":
      showNotice(() => errorText(message));
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
      return engine === "browser" ? t("errorNoModelBrowser") : t("errorNoModel");
    case "model_missing":
      return t("errorModelMissing");
    case "disconnected":
      return setupNeeded(message.message) ? t("noticeSetup") : t("noticeDisconnected", message.message ?? "");
    default:
      return t("errorFailed", message.message ?? message.code);
  }
}

/**
 * Explains why page.js could not be injected, offering the permission for all sites when it
 * would help.
 */
async function pageFailed(error) {
  // The extension may enter a tab after the toolbar button or the context menu was used on it
  // (activeTab), until the tab goes to another site; buttons in the panel do not grant that.
  // Other tabs need the permission for their site. Browser pages, the extension stores, and
  // error pages cannot be entered at all.
  const all = { origins: ["<all_urls>"] };
  const permissionHelps =
    /must request permission/i.test(error ?? "") && !(await chrome.permissions.contains(all).catch(() => false));
  if (!permissionHelps) {
    showNotice(() => t("pageCannotTranslate"));
    return;
  }
  showNotice(
    () => t("pageNotAllowed"),
    () => t("allowAllSites"),
    async () => {
      if (await chrome.permissions.request(all)) translatePage();
    },
  );
}

/** Whether the host could not be started: not registered, or the app was moved. */
function setupNeeded(error) {
  return /not found|forbidden|exited/i.test(error ?? "");
}

// ---- Rendering ----

const gb = (bytes) => `${(bytes / 1e9).toFixed(2)} GB`;

function renderOptions(options) {
  shownOptions = options;
  engine = options.engine ?? "app";
  renderEngineTexts();
  if (engine === "browser") {
    renderBrowserOptions(options);
    return;
  }
  $("browserModels").hidden = true;
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

/** The in-browser engine's models and devices: those that fit, downloaded or not. */
function renderBrowserOptions(options) {
  const downloaded = options.models.filter((m) => m.downloaded);
  const model = $("model");
  model.replaceChildren(new Option(t("modelAuto", downloaded[0]?.label ?? t("none")), ""));
  for (const m of options.models) {
    model.add(new Option(`${m.label} (${gb(m.bytes)})${m.downloaded ? "" : ` — ${t("notDownloaded")}`}`, m.id));
  }
  model.value = options.model ?? "";
  const device = $("device");
  device.replaceChildren(new Option(t("deviceAutoMeasured"), ""));
  for (const d of options.devices) device.add(new Option(`${d.name.toUpperCase()} — ${d.description}`, d.name));
  device.value = options.device && options.device !== "auto" ? options.device : "";
  renderBrowserModels();
}

/** The in-browser engine's models with their downloads, and the license before the first one. */
function renderBrowserModels() {
  const models = engine === "browser" ? (shownOptions?.models ?? []) : [];
  $("browserModels").hidden = models.length === 0;
  $("modelRows").replaceChildren(
    ...models.map((m) => {
      const row = document.createElement("div");
      row.className = "model-row";
      const name = document.createElement("span");
      name.className = "name";
      name.textContent = `${m.label} (${gb(m.bytes)})`;
      const state = document.createElement("span");
      state.className = "state";
      const button = document.createElement("button");
      button.className = "small";
      const download = downloads[m.id];
      if (download?.state === "running" || m.downloading) {
        state.textContent = t("downloading", String(Math.floor((100 * (download?.received ?? 0)) / m.bytes)));
        button.hidden = true;
      } else if (m.downloaded) {
        button.textContent = t("deleteModel");
        button.onclick = () => {
          if (confirm(t("deleteConfirm", m.label))) send({ type: "delete", model: m.id });
        };
      } else {
        if (download?.state === "error") {
          state.textContent = t("downloadFailed", download.message ?? "");
          state.classList.add("error");
        }
        button.textContent = t("download");
        button.onclick = () => {
          if (licenseAccepted) {
            send({ type: "download", model: m.id });
          } else {
            pendingDownload = m.id;
            renderBrowserModels();
          }
        };
      }
      row.append(name, state, button);
      return row;
    }),
  );
  $("license").hidden = !pendingDownload;
}

/** Texts that depend on the engine in use. */
function renderEngineTexts() {
  $("settingsHint").textContent = t(engine === "browser" ? "settingsHintBrowser" : "settingsHint");
  $("release").title = t(engine === "browser" ? "releaseTitleBrowser" : "releaseTitle");
  $("licenseLink").href = t("licenseUrl");
}

function renderStatus(status) {
  shownStatus = status;
  if (status.engine) engine = status.engine;
  let model = [status.model, status.device].filter(Boolean).join(" · ");
  if (engine === "browser" && model) model = t("statusInBrowser", model);
  $("release").disabled = status.state === "released" || status.state === "disconnected";
  switch (status.state) {
    case "ready":
      setStatus(model);
      break;
    case "loading":
    case "starting":
      setStatus(t("statusLoading", model));
      break;
    case "measuring":
      setStatus(t("statusMeasuring"));
      break;
    case "no_model":
      setStatus(t(engine === "browser" ? "statusNoModelBrowser" : "statusNoModel"), true);
      break;
    case "released":
      setStatus(status.reason === "idle" ? t("statusReleasedIdle") : t("statusReleased"), false, t("statusReleasedDetail"));
      break;
    case "disconnected":
      setStatus(t("statusDisconnected"), true);
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

/** Shows the progress line; `text` gives it in the current language. */
function setProgress(text) {
  progress = text;
  $("progress").textContent = text();
}

function setTranslating(translating) {
  const button = $("translate");
  button.textContent = translating ? t("stop") : t("translate");
  button.classList.toggle("primary", !translating);
  button.classList.toggle("danger", translating);
}

/**
 * Shows a notice, with a button running `action` if `label` is given. `text` and `label` give
 * their texts in the current language.
 */
function showNotice(text, label = null, action = null) {
  notice = { text, label, action };
  renderNotice();
}

function renderNotice() {
  $("noticeText").textContent = notice.text();
  const button = $("noticeAction");
  button.hidden = !notice.label;
  button.textContent = notice.label?.() ?? "";
  button.onclick = notice.action;
  $("notice").hidden = false;
}

function hideNotice() {
  notice = null;
  $("notice").hidden = true;
}

// Connect right away, with the saved choices, so the model loads while the user reads.
ready.then(() => send({ type: "options" }));
