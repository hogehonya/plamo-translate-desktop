// The extension's UI language, shared by the panel, the background, and page.js.
//
// "auto" (the default) follows the browser through chrome.i18n. A language chosen in the panel's
// settings (`uiLanguage` in chrome.storage.local) uses that locale's messages from _locales
// instead, with chrome.i18n as the fallback. The manifest's name and description always follow
// the browser.
//
// page.js is injected with this file each time a page translation starts, so this file declares
// only `var`s and functions, which may be declared again.

/** The languages offered besides "auto", by locale (a folder in _locales), in their own names. */
var UI_LANGUAGES = { en: "English", ja: "日本語" };

/** The chosen locale's messages (its messages.json), or null (or undefined) to follow the browser. */
var uiMessages;

/** Reads the chosen UI language and its messages. Returns the one in effect: "auto" or a locale. */
async function loadUiLanguage() {
  const { uiLanguage } = await chrome.storage.local.get({ uiLanguage: "auto" });
  uiMessages = Object.hasOwn(UI_LANGUAGES, uiLanguage) ? await readMessages(uiLanguage) : null;
  return uiMessages ? uiLanguage : "auto";
}

async function readMessages(locale) {
  try {
    const response = await fetch(chrome.runtime.getURL(`_locales/${locale}/messages.json`));
    return await response.json();
  } catch {
    return null;
  }
}

/** The locale the UI shows: the chosen one, or the one Chrome picked for the browser's language. */
function uiLocale() {
  return uiMessages?.locale?.message ?? chrome.i18n.getMessage("locale");
}

/**
 * The message `key` in the UI language, with its placeholders filled from `args` (strings) as
 * chrome.i18n.getMessage does: `$NAME$` names a placeholder whose content is `$1`, `$2`…
 */
function t(key, ...args) {
  const entry = uiMessages?.[key];
  if (!entry) return chrome.i18n.getMessage(key, args) || key;
  const arg = (n) => args[n - 1] ?? "";
  return entry.message.replace(/\$\$|\$(\d)|\$(\w+)\$/g, (match, n, name) => {
    if (match === "$$") return "$";
    if (n) return arg(Number(n));
    const content = entry.placeholders?.[name.toLowerCase()]?.content;
    return content === undefined ? match : content.replace(/\$(\d)/g, (_, m) => arg(Number(m)));
  });
}
