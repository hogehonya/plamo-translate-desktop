// Opens the side panel from the toolbar button and from the "Translate with PLaMo" context
// menu item, handing the selected text to the panel.

const MENU_ID = "translate-selection";

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: MENU_ID,
    title: chrome.i18n.getMessage("menuTranslate"),
    contexts: ["selection"],
  });
});

chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch(() => {});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId !== MENU_ID || !info.selectionText) return;
  // Must be called while handling the click, before anything asynchronous.
  chrome.sidePanel.open({ windowId: tab.windowId }).catch(() => {});
  // The panel picks this up when it opens, or right away if it is open already.
  chrome.storage.session.set({ request: { text: info.selectionText, at: Date.now() } });
});
