// Translates the page it is injected into (by background.js, on the user's request).
//
// The page is cut into runs: consecutive text and inline elements (links, emphasis, code…)
// within one block. Each run is sent as simple HTML whose elements carry an index instead of
// their attributes, such as `see the <a i="2">guide</a>`; the model keeps such tags in place.
// The translation is parsed inertly and rebuilt from text nodes and clones of the run's own
// elements (looked up by index), so links keep their targets and nothing the model writes can
// add attributes or scripts. The original nodes are kept to restore the page.

(() => {
  if (window.__plamoPage) {
    window.__plamoPage.start();
    return;
  }

  // Not translated, nor looked into.
  const SKIP = new Set([
    "SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "TEXTAREA", "INPUT", "SELECT", "OPTION", "BUTTON",
    "SVG", "MATH", "CANVAS", "IFRAME", "OBJECT", "VIDEO", "AUDIO", "PRE",
  ]);
  // Inline elements that can be part of a run.
  const INLINE = new Set([
    "A", "ABBR", "B", "BDI", "BDO", "BIG", "BR", "CITE", "CODE", "DATA", "DEL", "DFN", "EM", "FONT",
    "I", "IMG", "INS", "KBD", "LABEL", "MARK", "Q", "S", "SAMP", "SMALL", "SPAN", "STRIKE", "STRONG",
    "SUB", "SUP", "TIME", "TT", "U", "VAR", "WBR",
  ]);
  // Inline elements kept as they are: their content is not translated.
  const OPAQUE = new Set(["BR", "CODE", "IMG", "KBD", "SAMP", "VAR", "WBR"]);
  // Elements in a translation whose content is dropped along with them.
  const DROP = new Set(["SCRIPT", "STYLE", "TEMPLATE", "NOSCRIPT", "IFRAME", "OBJECT", "SVG", "MATH"]);
  const KANA = /[\u3040-\u309f\u30a0-\u30ff]/;
  const LETTER = /\p{L}/u;

  let port = null;
  let mode = "replace";
  let target = "ja";
  let style = "";
  let nextSid = 1;
  /** Runs sent and not yet translated, by sid. */
  const pending = new Map();
  /** Nodes of every run sent, so none is sent twice (the originals stay in "below" mode). */
  let handled = new WeakSet();
  /** Applied translations, to undo: { nodes (originals), added (inserted nodes) }. */
  const applied = [];
  let total = 0;
  let done = 0;
  let observer = null;
  let ui = null;
  /** Stopped by the user, with paragraphs left that Resume would translate. */
  let stopped = false;

  // ---- Collecting runs ----

  function ours(node) {
    return node.nodeType === Node.ELEMENT_NODE && node.hasAttribute("data-plamo");
  }

  function skipped(el) {
    return (
      SKIP.has(el.tagName) ||
      ours(el) ||
      el.isContentEditable ||
      el.getAttribute("translate") === "no" ||
      el.classList.contains("notranslate")
    );
  }

  /** Inline content kept as it is (code, line breaks, text marked not to translate). */
  function opaque(el) {
    return (
      OPAQUE.has(el.tagName) || el.getAttribute("translate") === "no" || el.classList.contains("notranslate")
    );
  }

  /** Whether `el` can be part of a run: inline, and with only inline content. */
  function inline(el) {
    if (!INLINE.has(el.tagName)) return false;
    if (opaque(el)) return true;
    if (skipped(el)) return false;
    for (const child of el.children) {
      if (!inline(child)) return false;
    }
    return true;
  }

  /** Runs of consecutive inline nodes under `root`, in document order. */
  function collect(root, out = []) {
    let run = [];
    const flush = () => {
      if (run.length && translatable(run)) out.push(run);
      run = [];
    };
    for (const node of root.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) {
        run.push(node);
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        if (node.hasAttribute("data-plamo-done")) {
          flush();
        } else if (inline(node)) {
          run.push(node);
        } else {
          flush();
          if (!skipped(node)) collect(node, out);
        }
      }
    }
    flush();
    return out;
  }

  function runText(run) {
    return run.map((n) => n.textContent).join("").replace(/\s+/g, " ").trim();
  }

  function translatable(run) {
    const text = runText(run);
    if (text.length < 2 || !LETTER.test(text)) return false;
    // Already Japanese. (Other languages cannot be told apart this cheaply.) Easy Japanese
    // rewrites Japanese too.
    return !(target === "ja" && KANA.test(text));
  }

  // ---- Serializing and rebuilding ----

  function escape(text) {
    return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  /** The run as indexed HTML, and the elements the indices refer to. */
  function serialize(run) {
    const elements = [];
    const write = (node) => {
      if (node.nodeType === Node.TEXT_NODE) return escape(node.data.replace(/\s+/g, " "));
      if (node.nodeType !== Node.ELEMENT_NODE) return "";
      const i = elements.push(node) - 1;
      const tag = node.tagName.toLowerCase();
      if (node.tagName === "BR" || node.tagName === "IMG" || node.tagName === "WBR") return `<${tag} i="${i}">`;
      const inner = opaque(node) ? escape(node.textContent) : [...node.childNodes].map(write).join("");
      return `<${tag} i="${i}">${inner}</${tag}>`;
    };
    return { html: run.map(write).join("").trim(), elements };
  }

  /** Nodes for `html` (the translation), built from text and clones of `elements`. */
  function rebuild(html, elements) {
    const doc = new DOMParser().parseFromString(`<body>${html}</body>`, "text/html");
    const used = new Set();
    const build = (source, into) => {
      for (const node of source.childNodes) {
        if (node.nodeType === Node.TEXT_NODE) {
          into.append(node.data);
        } else if (node.nodeType === Node.ELEMENT_NODE) {
          const i = Number(node.getAttribute("i"));
          const candidate = node.hasAttribute("i") && Number.isInteger(i) && !used.has(i) ? elements[i] : undefined;
          // The tag must match too, so a mislabelled index cannot turn emphasis into a link.
          const original = candidate?.tagName === node.tagName ? candidate : undefined;
          if (!original) {
            // Not one of the run's elements: never inserted. Its text is kept, except for
            // elements whose content is not text to show.
            if (!DROP.has(node.tagName)) build(node, into);
            continue;
          }
          used.add(i);
          const clone = original.cloneNode(opaque(original));
          // Below the original, the copy must not repeat its ids.
          if (mode === "below") {
            clone.removeAttribute("id");
            for (const el of clone.querySelectorAll("[id]")) el.removeAttribute("id");
          }
          if (!opaque(original)) build(node, clone);
          into.append(clone);
        }
      }
    };
    const fragment = document.createDocumentFragment();
    build(doc.body, fragment);
    return fragment;
  }

  function apply(item, text) {
    const { run, elements } = item;
    const first = run[0];
    if (!first?.isConnected) return;
    const fragment = rebuild(text, elements);
    // Text left as it was (names, code, text already in the target language) is not repeated
    // below the original, nor replaced by itself.
    if (fragment.textContent.replace(/\s+/g, " ").trim() === runText(run)) return;
    if (mode === "below") {
      const box = document.createElement("span");
      box.setAttribute("data-plamo", "");
      box.setAttribute("lang", target);
      box.style.cssText = "display:block;opacity:.78;margin:.2em 0 .4em;";
      box.append(fragment);
      const last = run[run.length - 1];
      last.after(box);
      applied.push({ nodes: [], added: [box] });
    } else {
      const holder = document.createElement("span");
      holder.setAttribute("data-plamo-done", "");
      holder.setAttribute("lang", target);
      // Keep the spacing around the run, which the translation does not have.
      const raw = run.map((n) => n.textContent).join("");
      holder.append(/^\s/.test(raw) ? " " : "", fragment, /\s$/.test(raw) ? " " : "");
      first.before(holder);
      for (const node of run) node.remove();
      applied.push({ nodes: run, added: [holder] });
    }
  }

  // ---- Talking to the background ----

  function connect() {
    port = chrome.runtime.connect({ name: "page" });
    port.onMessage.addListener((message) => {
      switch (message.type) {
        case "segment": {
          const item = pending.get(message.sid);
          if (!item) return;
          pending.delete(message.sid);
          apply(item, message.text);
          done++;
          render();
          break;
        }
        case "idle":
          pending.clear();
          render();
          break;
        case "stopped":
          // From the panel's button, or because another page started translating.
          stopLocally();
          render(message.reason === "panel" ? t("pageStopped") : t("pageStoppedForOther"));
          break;
        case "error":
          // Sent to every page when the connection ends; nothing to say if this one was done.
          if (!pending.size) break;
          // Resume sends the rest again.
          stopLocally();
          render(message.code === "released" ? t("pageStopped") : t("pageError", message.message || message.code));
          break;
      }
    });
    port.onDisconnect.addListener(() => {
      port = null;
      if (pending.size) {
        stopLocally();
        render(t("pageStopped"));
      }
    });
  }

  function send(runs, start = false) {
    runs = runs.filter((run) => !run.some((node) => handled.has(node)));
    if (!runs.length) return;
    for (const run of runs) for (const node of run) handled.add(node);
    if (!port) connect();
    // Visible paragraphs first, then the rest in reading order.
    const height = window.innerHeight;
    const position = (run) => {
      const top = anchor(run)?.getBoundingClientRect().top ?? 0;
      return top >= -50 && top < height ? top - 1e9 : top < 0 ? 1e9 - top : top;
    };
    const items = runs
      .map((run) => ({ run, position: position(run) }))
      .sort((a, b) => a.position - b.position)
      .map(({ run }) => {
        const { html, elements } = serialize(run);
        const sid = nextSid++;
        pending.set(sid, { run, elements });
        return { sid, text: html };
      });
    total += items.length;
    port.postMessage({ type: "segments", items, target, style, start });
    render();
  }

  // ---- Page UI ----

  function render(message) {
    if (!ui) {
      const host = document.createElement("div");
      host.setAttribute("data-plamo", "");
      host.style.cssText = "position:fixed;right:16px;bottom:16px;z-index:2147483647;";
      const root = host.attachShadow({ mode: "closed" });
      root.innerHTML = `
        <style>
          .box { font: 13px/1.5 system-ui, sans-serif; color: #1f2328; background: #fff;
                 border: 1px solid #d0d7de; border-radius: 8px; padding: 6px 8px;
                 box-shadow: 0 2px 8px rgba(0,0,0,.15); display: flex; gap: 8px; align-items: center; }
          @media (prefers-color-scheme: dark) { .box { color: #e6edf3; background: #161b22; border-color: #30363d; } }
          button { font: inherit; padding: 1px 8px; border-radius: 6px; border: 1px solid #d0d7de;
                   background: transparent; color: inherit; cursor: pointer; }
          b { color: #0d7fda; }
        </style>
        <div class="box"><b>PLaMo</b><span class="text"></span>
          <button class="stop"></button><button class="resume"></button><button class="undo"></button><button class="close">×</button></div>`;
      ui = {
        host,
        text: root.querySelector(".text"),
        stop: root.querySelector(".stop"),
        resume: root.querySelector(".resume"),
        undo: root.querySelector(".undo"),
        close: root.querySelector(".close"),
      };
      ui.resume.onclick = () => start();
      ui.stop.onclick = stop;
      ui.undo.onclick = undo;
      ui.close.onclick = () => {
        ui.host.remove();
        ui = null;
      };
      document.documentElement.append(host);
    }
    // Labelled on each render: the display language may change between page translations.
    ui.stop.textContent = t("stop");
    ui.resume.textContent = t("pageResume");
    ui.undo.textContent = t("pageUndo");
    const working = pending.size > 0;
    ui.text.textContent = message ?? (working ? t("pageProgress", String(done), String(total)) : t("pageDone", String(done)));
    ui.stop.hidden = !working;
    ui.resume.hidden = working || !stopped;
    ui.undo.hidden = applied.length === 0;
  }

  function stop() {
    port?.postMessage({ type: "cancel" });
    stopLocally();
    render(t("pageStopped"));
  }

  /** Forgets the paragraphs waiting, which Resume can send again. */
  function stopLocally() {
    for (const item of pending.values()) for (const node of item.run) handled.delete(node);
    stopped = pending.size > 0;
    pending.clear();
    observer?.disconnect();
    observer = null;
  }

  /** The element whose position stands for `run`. */
  function anchor(run) {
    return run[0].nodeType === Node.ELEMENT_NODE ? run[0] : run[0].parentElement;
  }

  // When scrolling stops, the paragraphs in view move to the front of the queue.
  let scrollTimer = null;
  function onScroll() {
    if (!pending.size || !port) return;
    clearTimeout(scrollTimer);
    scrollTimer = setTimeout(() => {
      const height = window.innerHeight;
      const visible = [...pending.entries()]
        .map(([sid, item]) => ({ sid, rect: anchor(item.run)?.getBoundingClientRect() }))
        .filter(({ rect }) => rect && rect.bottom > -100 && rect.top < height + 100)
        .sort((a, b) => a.rect.top - b.rect.top)
        .map(({ sid }) => sid);
      if (visible.length) port?.postMessage({ type: "prioritize", sids: visible });
    }, 250);
  }
  // Captured, so scrolling inside containers counts too.
  document.addEventListener("scroll", onScroll, { capture: true, passive: true });

  function undo() {
    stop();
    for (const { nodes, added } of applied.reverse()) {
      if (nodes.length) added[0].before(...nodes);
      for (const node of added) node.remove();
    }
    applied.length = 0;
    handled = new WeakSet();
    total = done = 0;
    render(t("pageRestored"));
  }

  // ---- Start ----

  async function start() {
    stopped = false;
    const [settings, messages] = await Promise.all([
      chrome.storage.local.get({ target: "auto", style: "", pageMode: "replace" }),
      // The display language's messages, for t() in i18n.js (injected with this file).
      chrome.runtime.sendMessage({ type: "ui_messages" }).catch(() => null),
    ]);
    uiMessages = messages;
    mode = settings.pageMode;
    style = settings.style;
    target = settings.target;
    if (target === "auto") {
      // Japanese pages go into English, everything else into Japanese.
      const sample = (document.body.innerText || "").slice(0, 20000);
      const kana = (sample.match(/[\u3040-\u30ff]/g) || []).length;
      target = kana > sample.length * 0.05 ? "en" : "ja";
    }
    if (target !== "ja") style = "";
    send(collect(document.body), true);
    // Also when nothing new was sent (the page is translated already), so that the box shows
    // again, even if it was closed: the request was not ignored.
    render(total ? undefined : t("pageNothing"));
    // Paragraphs added later (infinite scrolling, expanding sections).
    observer?.disconnect();
    let timer = null;
    const added = new Set();
    observer = new MutationObserver((mutations) => {
      for (const m of mutations) {
        for (const node of m.addedNodes) {
          if (node.nodeType === Node.ELEMENT_NODE && !ours(node) && !node.closest("[data-plamo],[data-plamo-done]")) {
            added.add(node);
          }
        }
      }
      clearTimeout(timer);
      timer = setTimeout(() => {
        const roots = [...added].filter((n) => n.isConnected);
        added.clear();
        // Overlapping roots find a run twice; send() drops those already sent.
        const runs = roots.flatMap((root) => (inline(root) ? collect(root.parentElement ?? root) : collect(root)));
        send(runs);
      }, 500);
    });
    observer.observe(document.body, { childList: true, subtree: true });
  }

  window.__plamoPage = { start };
  start();
})();
