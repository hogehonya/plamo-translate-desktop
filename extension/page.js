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
  /** Tokens per second of the last batch, from the background. */
  let speed = null;
  /** Stopped by the user, with paragraphs left that Resume would translate. */
  let stopped = false;
  /**
   * Runs found but not sent yet: hidden ones (collapsed sections, menus, text for screen
   * readers), and with the panel's setting those far from the view. They are sent when they
   * show or come near (see `recheck`).
   */
  let deferred = [];
  /** The panel's setting: only the runs near the view are translated, the rest when scrolled to. */
  let nearbyOnly = false;
  let recheckTimer = null;

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
          drawMarks();
          break;
        }
        case "idle":
          // After a stop or a failure, the box keeps saying so.
          if (stopped) break;
          pending.clear();
          setWorking([]);
          render();
          break;
        case "speed":
          speed = message.tokensPerSecond;
          render();
          break;
        case "working":
          setWorking(message.sids);
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
    const progress = working ? t("pageProgress", String(done), String(total)) : t("pageDone", String(done));
    const rate = speed ? ` · ${t("liveSpeed", speed.toFixed(1))}` : "";
    ui.text.textContent = message ?? progress + rate;
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
    stopped = pending.size > 0 || deferred.length > 0;
    pending.clear();
    deferred = [];
    setWorking([]);
    observer?.disconnect();
    observer = null;
    clearInterval(recheckTimer);
    recheckTimer = null;
  }

  // ---- What is sent: shown, and with the setting near the view ----

  /**
   * Whether the run shows on the page: its nodes not hidden by their styles (display,
   * visibility, opacity), and not shrunk to nothing (text for screen readers only).
   */
  function shown(run) {
    const options = { checkOpacity: true, checkVisibilityCSS: true };
    const styled = run.some((node) => {
      const el = node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
      return el?.checkVisibility ? el.checkVisibility(options) : true;
    });
    if (!styled) return false;
    const rect = runRect(run);
    return !!rect && rect.width > 2 && rect.height > 2;
  }

  /** Whether the run is within a screen's height of the view. */
  function near(run) {
    const rect = runRect(run);
    const height = window.innerHeight;
    return !!rect && rect.bottom > -height && rect.top < 2 * height;
  }

  const wanted = (run) => shown(run) && (!nearbyOnly || near(run));

  /** Sends the runs wanted now, keeping the others for later. */
  function offer(runs, start = false) {
    const now = [];
    for (const run of runs) (wanted(run) ? now : deferred).push(run);
    send(now, start);
  }

  /** Sends the deferred runs that show or came near since. */
  function recheck() {
    if (!deferred.length || !observer) return;
    deferred = deferred.filter((run) => run[0].isConnected && !run.some((node) => handled.has(node)));
    const now = deferred.filter(wanted);
    if (!now.length) return;
    deferred = deferred.filter((run) => !now.includes(run));
    send(now);
  }

  // ---- The runs being translated ----

  // The background tells which runs the engine is on (`working`): up to one per batch running.
  // A pulsing frame over each, in a layer of our own above the page (the page is not touched),
  // shows where the translation is while it takes its time (on the CPU, for example). The layer
  // is placed in the document, not fixed to the view, so that the frames scroll with the page
  // (a fixed one lags while the browser scrolls); runs in scrolled containers are followed by
  // redrawing on their scroll events.
  let working = [];
  /** The panel's setting for the frames (on by default). */
  let showMarks = true;
  /** The layer: { host, root, frames: Map(sid → element) }. */
  let marks = null;
  let marksTimer = null;

  function setWorking(sids) {
    working = sids.filter((sid) => pending.has(sid));
    drawMarks();
    // The page moves under the frames as translations replace text: follow it.
    if (working.length && !marksTimer) marksTimer = setInterval(drawMarks, 500);
    if (!working.length && marksTimer) {
      clearInterval(marksTimer);
      marksTimer = null;
    }
  }

  function drawMarks() {
    if (!showMarks) {
      marks?.host.remove();
      marks = null;
      return;
    }
    if (!marks) {
      if (!working.length) return;
      const host = document.createElement("div");
      host.setAttribute("data-plamo", "");
      host.style.cssText = "position:absolute;top:0;left:0;width:0;height:0;pointer-events:none;z-index:2147483646;";
      const root = host.attachShadow({ mode: "closed" });
      root.innerHTML = `
        <style>
          .frame { position: absolute; box-sizing: border-box; border: 2px solid rgba(13, 127, 218, .75);
                   border-radius: 4px; background: rgba(13, 127, 218, .07); animation: pulse 1.6s ease-in-out infinite; }
          @keyframes pulse { 50% { border-color: rgba(13, 127, 218, .25); background: rgba(13, 127, 218, .02); } }
          @media (prefers-reduced-motion: reduce) { .frame { animation: none; } }
        </style>`;
      document.documentElement.append(host);
      marks = { host, root, frames: new Map() };
    }
    for (const [sid, frame] of marks.frames) {
      if (working.includes(sid) && pending.has(sid)) continue;
      frame.remove();
      marks.frames.delete(sid);
    }
    // Positions relative to the layer, wherever the page's styles put its origin.
    const origin = marks.host.getBoundingClientRect();
    for (const sid of working) {
      const rect = pending.has(sid) ? runRect(pending.get(sid).run) : null;
      let frame = marks.frames.get(sid);
      if (!rect) {
        frame?.remove();
        marks.frames.delete(sid);
        continue;
      }
      if (!frame) {
        frame = document.createElement("div");
        frame.className = "frame";
        marks.root.append(frame);
        marks.frames.set(sid, frame);
      }
      Object.assign(frame.style, {
        left: `${rect.left - origin.left - 4}px`,
        top: `${rect.top - origin.top - 3}px`,
        width: `${rect.width + 8}px`,
        height: `${rect.height + 6}px`,
      });
    }
  }

  /** Where the run's nodes are on screen, or null if they are gone or take no space. */
  function runRect(run) {
    if (!run[0]?.isConnected || !run[run.length - 1].isConnected) return null;
    const range = document.createRange();
    range.setStartBefore(run[0]);
    range.setEndAfter(run[run.length - 1]);
    const rect = range.getBoundingClientRect();
    return rect.width || rect.height ? rect : null;
  }

  let marksFrame = 0;
  const redrawMarks = () => {
    if (!working.length || marksFrame) return;
    marksFrame = requestAnimationFrame(() => {
      marksFrame = 0;
      drawMarks();
    });
  };
  document.addEventListener("scroll", redrawMarks, { capture: true, passive: true });
  window.addEventListener("resize", redrawMarks, { passive: true });
  // Turned on or off in the panel, also while the page translates.
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area !== "local") return;
    if (changes.pageMarks) {
      showMarks = changes.pageMarks.newValue !== false;
      drawMarks();
    }
    if (changes.pageNearby) {
      nearbyOnly = changes.pageNearby.newValue === true;
      recheck();
    }
  });

  /** The element whose position stands for `run`. */
  function anchor(run) {
    return run[0].nodeType === Node.ELEMENT_NODE ? run[0] : run[0].parentElement;
  }

  // When scrolling stops, the paragraphs in view move to the front of the queue, and those that
  // came near are sent.
  let scrollTimer = null;
  function onScroll() {
    if (!pending.size && !deferred.length) return;
    clearTimeout(scrollTimer);
    scrollTimer = setTimeout(() => {
      recheck();
      if (!pending.size || !port) return;
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
    deferred = [];
    total = done = 0;
    render(t("pageRestored"));
  }

  // ---- Start ----

  async function start() {
    stopped = false;
    speed = null;
    const [settings, messages] = await Promise.all([
      chrome.storage.local.get({ target: "auto", style: "", pageMode: "replace", pageMarks: true, pageNearby: false }),
      // The display language's messages, for t() in i18n.js (injected with this file).
      chrome.runtime.sendMessage({ type: "ui_messages" }).catch(() => null),
    ]);
    uiMessages = messages;
    mode = settings.pageMode;
    showMarks = settings.pageMarks !== false;
    nearbyOnly = settings.pageNearby === true;
    style = settings.style;
    target = settings.target;
    if (target === "auto") {
      // Japanese pages go into English, everything else into Japanese.
      const sample = (document.body.innerText || "").slice(0, 20000);
      const kana = (sample.match(/[\u3040-\u30ff]/g) || []).length;
      target = kana > sample.length * 0.05 ? "en" : "ja";
    }
    if (target !== "ja") style = "";
    deferred = [];
    offer(collect(document.body), true);
    // Also when nothing new was sent (the page is translated already), so that the box shows
    // again, even if it was closed: the request was not ignored.
    render(total || deferred.length ? undefined : t("pageNothing"));
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
        offer(runs.filter((run) => !deferred.some((d) => d[0] === run[0])));
      }, 500);
    });
    observer.observe(document.body, { childList: true, subtree: true });
    // Sections that open, or tabs that switch, show runs without adding nodes.
    clearInterval(recheckTimer);
    recheckTimer = setInterval(recheck, 2000);
  }

  window.__plamoPage = { start };
  start();
})();
