// The in-browser engine: runs PLaMo 3 Translate with wllama (llama.cpp compiled to WebAssembly,
// on the CPU or through WebGPU) in this offscreen document. The background opens it when the
// extension translates without the desktop app, and closes it after a while without use, which
// frees the model's memory (see background.js).
//
// It speaks the host's protocol (crates/host/src/protocol.rs) over a port named "engine", with
// these differences:
// - The models are the ones that fit WebAssembly's memory, downloaded into the extension's
//   storage (OPFS): `download { model }` fetches one, reporting `download` messages.
// - The devices are `cpu` and `gpu` (WebGPU), and `auto` measures which is faster.
// - Status and options carry `engine: "browser"`.

import { Wllama } from "./vendor/wllama/wllama.js";
import { History, STOP, estimateTokens, prompt, resolveTarget, split } from "./translator.js";

/**
 * The models offered: the app's catalog entries that fit WebAssembly's 4 GB of memory, at the
 * revision of 2026-11-06 (the files of the first revision are no longer served).
 */
const MODELS = [
  {
    id: "plamo-3-translate-2b-Q4_K_M",
    label: "2B · Q4_K_M",
    repo: "pfnet/plamo-3-translate-2b-GGUF",
    revision: "232fbd7916519edecddfdae0beae7e76fe70453f",
    bytes: 1807199392,
  },
  {
    id: "plamo-3-translate-0.8b-Q4_K_M",
    label: "0.8B · Q4_K_M",
    repo: "pfnet/plamo-3-translate-0.8b-GGUF",
    revision: "dbeb0856cb0b004b484f190bb71dd65876297013",
    bytes: 573123200,
  },
];
const fileName = (model) => `${model.id}.gguf`;
const modelUrl = (model) => `https://huggingface.co/${model.repo}/resolve/${model.revision}/${fileName(model)}`;

// Smaller than the app's: WebAssembly reads prompts several times slower than native code, so
// shorter pieces and history keep each request quick.
const N_CTX = 4096;
const CHUNK_TOKENS = 512;
const HISTORY_TOKENS = 768;

// llama.cpp's WebGPU backend requests an adapter without a power preference, which gets the
// integrated GPU beside a discrete one. wllama's worker code passes through
// URL.createObjectURL as a Blob, so it is prefixed here to ask for the high-performance GPU.
const GPU_PREFERENCE = `if (typeof GPU !== "undefined") {
  const request = GPU.prototype.requestAdapter;
  GPU.prototype.requestAdapter = function (options = {}) {
    return request.call(this, { ...options, powerPreference: options.powerPreference ?? "high-performance" });
  };
}
`;
const createObjectURL = URL.createObjectURL;
URL.createObjectURL = (object) =>
  createObjectURL(
    object instanceof Blob && /javascript/.test(object.type) ? new Blob([GPU_PREFERENCE, object], { type: object.type }) : object,
  );

const port = chrome.runtime.connect({ name: "engine" });
const post = (message) => port.postMessage(message);

/** The extension's choices ("" or absent: the default; 0 threads: wllama's default). */
let settings = { model: "", device: "auto", threads: 0 };
/** The model in use: { model, device, wllama }, or null. */
let loaded = null;
/** Settles when the current load (or measurement) ends. */
let loading = null;
/** The running translation: { id, controller }. */
let job = null;
// "starting" until the downloaded models are checked (see the end of this file).
let status = { state: "starting" };
const downloads = new Map();

// ---- Storage ----

async function modelsDir() {
  const root = await navigator.storage.getDirectory();
  return root.getDirectoryHandle("models", { create: true });
}

async function isDownloaded(model) {
  try {
    const file = await (await (await modelsDir()).getFileHandle(fileName(model))).getFile();
    return file.size === model.bytes;
  } catch {
    return false;
  }
}

/** The chosen model, else the first one downloaded. */
async function selectedModel() {
  const chosen = MODELS.find((m) => m.id === settings.model);
  if (chosen) return chosen;
  for (const model of MODELS) if (await isDownloaded(model)) return model;
  return null;
}

/** Downloads `model` into OPFS, resuming a partial download. */
async function download(model) {
  if (downloads.has(model.id)) return;
  downloads.set(model.id, true);
  const report = (extra) => post({ type: "download", model: model.id, total: model.bytes, ...extra });
  try {
    const dir = await modelsDir();
    const partName = `${fileName(model)}.part`;
    const part = await dir.getFileHandle(partName, { create: true });
    const existing = (await part.getFile()).size;
    const response = await fetch(modelUrl(model), existing ? { headers: { Range: `bytes=${existing}-` } } : {});
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const resume = response.status === 206;
    const writable = await part.createWritable({ keepExistingData: resume });
    let received = 0;
    if (resume) {
      await writable.seek(existing);
      received = existing;
    }
    let reported = 0;
    const reader = response.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      await writable.write(value);
      received += value.length;
      if (received - reported > 8 << 20) {
        reported = received;
        report({ state: "running", received });
      }
    }
    await writable.close();
    if (received !== model.bytes) {
      throw new Error(`downloaded ${received} bytes instead of ${model.bytes}`);
    }
    await part.move(dir, fileName(model));
    report({ state: "done", received });
  } catch (e) {
    report({ state: "error", message: String(e?.message ?? e) });
  } finally {
    downloads.delete(model.id);
  }
  sendOptions();
}

async function deleteModel(model) {
  if (loaded?.model === model) await unload();
  const dir = await modelsDir();
  for (const name of [fileName(model), `${fileName(model)}.part`]) {
    await dir.removeEntry(name).catch(() => {});
  }
  sendOptions();
}

// ---- Devices ----

/** The WebGPU adapter wllama would get, described, or null without one. */
async function gpuDescription() {
  const adapter = await navigator.gpu?.requestAdapter({ powerPreference: "high-performance" }).catch(() => null);
  if (!adapter) return null;
  const info = adapter.info ?? {};
  return info.description || [info.vendor, info.architecture].filter(Boolean).join(" ") || "WebGPU";
}

/** What `auto` measured on this machine, kept while the GPU and the cores are the same. */
function measuredDevice(hardware) {
  try {
    const saved = JSON.parse(localStorage.getItem("measured") ?? "null");
    return saved?.hardware === hardware ? saved.device : null;
  } catch {
    return null;
  }
}

// Two texts: the second is timed, and must differ from the first, whose beginning llama.cpp
// would otherwise reuse instead of reading the prompt again.
const WARM_UP_TEXT = "The meeting has been moved to next Monday.";
const MEASURE_TEXT =
  "Large language models have changed how software teams handle documentation. Instead of writing " +
  "every page by hand, engineers now draft the structure, let a model fill in routine sections, and " +
  "spend their time reviewing technical details. This shift raises new questions about accuracy, " +
  "ownership and style, and about who checks the result before it is published.";

/**
 * Seconds for a typical piece (200 prompt and 100 generated tokens) on the loaded model, and the
 * speeds measured.
 */
async function measure() {
  const run = (text) =>
    loaded.wllama.createCompletion({ prompt: prompt({ text, target: "ja" }), max_tokens: 64, temperature: 0, stop: [STOP] });
  // The first run includes one-time work, such as compiling GPU kernels.
  await run(WARM_UP_TEXT);
  const { timings } = await run(MEASURE_TEXT);
  return {
    seconds: 200 / timings.prompt_per_second + 100 / timings.predicted_per_second,
    prompt: Math.round(timings.prompt_per_second),
    generation: Math.round(timings.predicted_per_second * 10) / 10,
  };
}

// ---- Loading ----

async function load(model, device) {
  if (loaded?.model === model && loaded.device === device && loaded.requestedThreads === settings.threads) return;
  await unload();
  setStatus({ state: "loading", model: model.label, device: device === "gpu" ? "WebGPU" : "CPU" });
  const file = await (await (await modelsDir()).getFileHandle(fileName(model))).getFile();
  // llama.cpp's messages while loading tell which device it really uses.
  const lines = [];
  const keep = (...args) => lines.push(args.join(" "));
  const wllama = new Wllama({ default: "./vendor/wllama/wllama.wasm" }, {
    // The layers offloaded are logged at the debug level, the adapter at the log level.
    logger: { debug: keep, log: keep, info: keep, warn: keep, error: (...args) => (keep(...args), console.error(...args)) },
  });
  try {
    await wllama.loadModel([file], {
      n_ctx: N_CTX,
      n_gpu_layers: device === "gpu" ? 999 : 0,
      ...(settings.threads > 0 ? { n_threads: settings.threads } : {}),
    });
  } catch (e) {
    await wllama.exit().catch(() => {});
    throw e;
  }
  loaded = {
    model,
    device,
    wllama,
    gpu: device === "gpu" ? usedGpu(lines) : null,
    requestedThreads: settings.threads,
    threads: wllama.getNumThreads(),
  };
  if (device === "gpu" && !loaded.gpu) console.warn("WebGPU was not used:", lines.filter((l) => /webgpu|offload/i.test(l)).join("\n"));
}

/**
 * The GPU llama.cpp computes on, from its messages while loading (its adapter, and the layers
 * offloaded), or null: without an adapter in wllama's worker it quietly computes on the CPU.
 */
function usedGpu(lines) {
  const adapter = lines.find((l) => l.includes("ggml_webgpu: adapter_info:"));
  const offloaded = lines.map((l) => /offloaded (\d+)\/\d+ layers to GPU/.exec(l)).find(Boolean);
  if (!adapter || !offloaded || offloaded[1] === "0") return null;
  const field = (name) => new RegExp(`${name}: ([^|]*)`).exec(adapter)?.[1].trim();
  return [field("vendor"), field("architecture")].filter(Boolean).join(" ") || "WebGPU";
}

/** The device in use, as the status shows it: `WebGPU (amd rdna-4)`, or `CPU`. */
const deviceLabel = () => (loaded?.gpu ? `WebGPU (${loaded.gpu})` : "CPU");

async function unload() {
  const current = loaded;
  loaded = null;
  await current?.wllama.exit().catch(() => {});
}


/**
 * Loads the selected model on the selected device. On `auto` with a GPU, the first load measures
 * the GPU and the CPU and keeps the faster one.
 */
async function ensureLoaded() {
  while (loading) await loading;
  const model = await selectedModel();
  if (!model || !(await isDownloaded(model))) {
    setStatus({ state: "no_model" });
    throw Object.assign(new Error("no model is downloaded"), { code: "no_model" });
  }
  let device = settings.device === "cpu" || settings.device === "gpu" ? settings.device : null;
  const gpu = device === "cpu" ? null : await gpuDescription();
  if (device === "gpu" && !gpu) device = "cpu";
  const hardware = `${gpu}; ${navigator.hardwareConcurrency} cores`;
  device ??= gpu ? measuredDevice(hardware) : "cpu";
  loading = (async () => {
    if (device) {
      await load(model, device);
      return;
    }
    // Measure: the GPU, then the CPU, keeping the faster one loaded.
    setStatus({ state: "measuring", model: model.label });
    const failed = { seconds: Infinity };
    await load(model, "gpu");
    // Without the GPU in use, this would measure the CPU.
    const gpuSpeed = loaded.gpu ? await measure().catch(() => failed) : failed;
    await load(model, "cpu");
    const cpuSpeed = await measure().catch(() => failed);
    const faster = gpuSpeed.seconds < cpuSpeed.seconds ? "gpu" : "cpu";
    localStorage.setItem("measured", JSON.stringify({ hardware, device: faster, gpu: gpuSpeed, cpu: cpuSpeed }));
    if (faster === "gpu") await load(model, "gpu");
  })();
  try {
    await loading;
    // `gpuUnavailable`: WebGPU was chosen (or measured) but llama.cpp could not use it.
    setStatus({
      state: "ready",
      model: model.label,
      device: deviceLabel(),
      // The CPU's threads, which the panel shows.
      threads: loaded.gpu ? undefined : loaded.threads,
      gpuUnavailable: loaded.device === "gpu" && !loaded.gpu,
    });
  } catch (e) {
    setStatus({ state: "error", model: model.label, message: String(e?.message ?? e) });
    throw Object.assign(new Error(String(e?.message ?? e)), { code: "load_failed" });
  } finally {
    loading = null;
  }
}

// ---- Translating ----

/** Starts a job, ending the running one (it reports itself cancelled). */
async function startJob(id) {
  const previous = job;
  job = { id, controller: new AbortController() };
  previous?.controller.abort();
  // Wait for it to report done, so that messages keep their order.
  await previous?.running;
  return job;
}

/** Generates the translation for `text` (a prompt), streaming pieces to `onText`. */
async function generate(current, text, maxTokens, onText) {
  let output = "";
  let tokens = 0;
  let finish = null;
  await loaded.wllama.createCompletion({
    prompt: text,
    max_tokens: maxTokens,
    temperature: 0,
    stop: [STOP],
    stream: true,
    abortSignal: current.controller.signal,
    onData: (chunk) => {
      const choice = chunk.choices?.[0];
      tokens++;
      finish = choice?.finish_reason ?? finish;
      if (choice?.text) {
        output += choice.text;
        onText(choice.text);
      }
    },
  });
  return { output, tokens, cut: finish === "length" };
}

/** The output budget for a piece: generous, so that only output running away is cut. */
const maxTokensFor = (source) => Math.min(N_CTX / 2, estimateTokens(source) * 3 + 64);

async function run(id, work) {
  const current = await startJob(id);
  let finished;
  current.running = new Promise((resolve) => (finished = resolve));
  const totals = { tokens: 0, cut: 0 };
  try {
    await ensureLoaded();
    if (current.controller.signal.aborted) throw new DOMException("cancelled", "AbortError");
    await work(current, totals);
    post({ type: "done", id, tokens: totals.tokens, cut: totals.cut, stopped: totals.cut > 0, cancelled: false });
  } catch (e) {
    if (current.controller.signal.aborted) {
      post({ type: "done", id, tokens: totals.tokens, cut: totals.cut, stopped: true, cancelled: true });
    } else {
      post({ type: "error", id, code: e.code ?? "failed", message: String(e?.message ?? e) });
    }
  } finally {
    if (job === current) job = null;
    finished();
  }
}

function translate(request) {
  return run(request.id, async (current, totals) => {
    const target = resolveTarget(request.target, request.text);
    post({ type: "started", id: request.id, target });
    const { leading, chunks } = split(request.text, CHUNK_TOKENS);
    if (leading) post({ type: "text", id: request.id, text: leading });
    const history = new History(HISTORY_TOKENS);
    for (const [index, chunk] of chunks.entries()) {
      if (chunks.length > 1) post({ type: "progress", id: request.id, index, count: chunks.length });
      const text = prompt({ text: chunk.text, target, source: request.source, style: request.style, history: history.pairs });
      const result = await generate(current, text, maxTokensFor(chunk.text), (piece) =>
        post({ type: "text", id: request.id, text: piece }),
      );
      totals.tokens += result.tokens;
      if (result.cut) totals.cut++;
      post({ type: "text", id: request.id, text: chunk.separator });
      history.add(chunk.text, result.output.trim());
    }
  });
}

function translateSegments(request) {
  return run(request.id, async (current, totals) => {
    const target = resolveTarget(request.target, request.segments.join("\n"));
    post({ type: "started", id: request.id, target });
    const history = new History(HISTORY_TOKENS);
    for (const [index, segment] of request.segments.entries()) {
      const text = prompt({ text: segment, target, source: request.source, style: request.style, history: history.pairs });
      const result = await generate(current, text, maxTokensFor(segment), () => {});
      totals.tokens += result.tokens;
      if (result.cut) totals.cut++;
      const translation = result.output.trim();
      post({ type: "segment", id: request.id, index, text: translation });
      history.add(segment, translation);
    }
  });
}

// ---- Status and options ----

function setStatus(next) {
  status = next;
  sendStatus();
}

function sendStatus() {
  post({ type: "status", engine: "browser", ...status });
}

async function sendOptions() {
  const gpu = await gpuDescription();
  const models = [];
  for (const model of MODELS) {
    models.push({ id: model.id, label: model.label, bytes: model.bytes, downloaded: await isDownloaded(model), downloading: downloads.has(model.id) });
  }
  post({
    type: "options",
    engine: "browser",
    models,
    devices: [
      { name: "cpu", description: `WebAssembly, ${navigator.hardwareConcurrency} threads`, kind: "cpu" },
      ...(gpu ? [{ name: "gpu", description: gpu, kind: "gpu" }] : []),
    ],
    model: settings.model || null,
    device: settings.device || null,
    // The thread counts to choose from go up to the logical cores.
    cores: navigator.hardwareConcurrency,
    threads: settings.threads || null,
  });
}

// ---- Messages ----

port.onMessage.addListener(async (message) => {
  switch (message.type) {
    case "configure":
      settings = { model: message.model ?? "", device: message.device ?? "auto", threads: Number(message.threads) || 0 };
      // The next translation loads what is chosen now.
      if (
        loaded &&
        (loaded.model.id !== (await selectedModel())?.id ||
          (settings.device !== "auto" && settings.device !== loaded.device) ||
          (!loaded.gpu && settings.threads !== loaded.requestedThreads))
      ) {
        await unload();
      }
      sendStatus();
      return;
    case "status":
      sendStatus();
      return;
    case "options":
      sendOptions();
      return;
    case "download": {
      const model = MODELS.find((m) => m.id === message.model);
      if (model) download(model);
      return;
    }
    case "delete": {
      const model = MODELS.find((m) => m.id === message.model);
      if (model) deleteModel(model);
      return;
    }
    case "cancel":
      job?.controller.abort();
      return;
    case "translate":
      translate(message);
      return;
    case "translate_segments":
      translateSegments(message);
      return;
  }
});

// Chrome stops an idle service worker, which would drop this port; it is kept alive by messages.
// The background closes this document after a while without work.
setInterval(() => post({ type: "keepalive" }), 20_000);

// Ask for the persistent storage that keeps the models when space runs short.
navigator.storage.persist?.().catch(() => {});
(async () => {
  const model = await selectedModel();
  setStatus(model ? { state: "released", model: model.label } : { state: "no_model" });
})();
