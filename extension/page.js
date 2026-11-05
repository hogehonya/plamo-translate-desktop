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

  const t = (key, ...args) => chrome.i18n.getMessage(key, args) || key;

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
    // Already Japanese. (Other languages cannot be told apart this cheaply.)
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
          const original = Number.isInteger(i) && !used.has(i) ? elements[i] : undefined;
          if (!original) {
            build(node, into); // Unknown or repeated: keep only its text.
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
        case "error":
          pending.clear();
          render(message.code === "released" ? t("pageStopped") : t("pageError", message.message || message.code));
          break;
      }
    });
    port.onDisconnect.addListener(() => {
      port = null;
      if (pending.size) {
        pending.clear();
        render(t("pageStopped"));
      }
    });
  }

  function send(runs) {
    runs = runs.filter((run) => !run.some((node) => handled.has(node)));
    if (!runs.length) return;
    for (const run of runs) for (const node of run) handled.add(node);
    if (!port) connect();
    // Visible paragraphs first, then the rest in reading order.
    const height = window.innerHeight;
    const position = (run) => {
      const el = run[0].nodeType === Node.ELEMENT_NODE ? run[0] : run[0].parentElement;
      const top = el?.getBoundingClientRect().top ?? 0;
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
    port.postMessage({ type: "segments", items, target, style });
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
          <button class="stop"></button><button class="undo"></button><button class="close">×</button></div>`;
      ui = {
        host,
        text: root.querySelector(".text"),
        stop: root.querySelector(".stop"),
        undo: root.querySelector(".undo"),
        close: root.querySelector(".close"),
      };
      ui.stop.textContent = t("stop");
      ui.undo.textContent = t("pageUndo");
      ui.stop.onclick = stop;
      ui.undo.onclick = undo;
      ui.close.onclick = () => {
        ui.host.remove();
        ui = null;
      };
      document.documentElement.append(host);
    }
    const working = pending.size > 0;
    ui.text.textContent = message ?? (working ? t("pageProgress", String(done), String(total)) : t("pageDone", String(done)));
    ui.stop.hidden = !working;
    ui.undo.hidden = applied.length === 0;
  }

  function stop() {
    port?.postMessage({ type: "cancel" });
    pending.clear();
    observer?.disconnect();
    observer = null;
    render(t("pageStopped"));
  }

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
    const settings = await chrome.storage.local.get({ target: "auto", style: "", pageMode: "replace" });
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
    send(collect(document.body));
    if (!total) render(t("pageNothing"));
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
