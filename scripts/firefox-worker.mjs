// Firefox MV3 disallows blob workers. Materialize the bundled wllama worker at
// build time, with a fixed single-thread CPU configuration. No remote code or
// relaxed browser security settings are needed. Fail if upstream layout changes.
import fs from 'node:fs';
import vm from 'node:vm';
const [input, output] = process.argv.slice(2);
const source = fs.readFileSync(input, 'utf8');
function bundledString(name) {
  const start = source.indexOf(`var ${name} = `);
  const end = source.indexOf('\n', start);
  if (start < 0 || end < 0) throw new Error(`Missing upstream worker constant: ${name}`);
  return vm.runInNewContext(`${source.slice(start, end)}\n${name}`, {}, { timeout: 1000 });
}
const stubStart = source.indexOf('var JSPI_STUB = `');
const stubEnd = source.indexOf('\n`;', stubStart);
if (stubStart < 0 || stubEnd < 0) throw new Error('Missing upstream JSPI stub');
const stub = vm.runInNewContext(`${source.slice(stubStart, stubEnd + 3)}\nJSPI_STUB`, {}, { timeout: 1000 });
const moduleCode = stub + bundledString('WLLAMA_EMSCRIPTEN_CODE');
const worker = [
  '// Generated from the bundled MIT-licensed wllama and llama.cpp. See their LICENSE files.',
  'const RUN_OPTIONS = {pathConfig: {"wllama.wasm": new URL("wllama.wasm", self.location.href).href}, nbThread: 0, compat: false};',
  'try { Object.defineProperty(WorkerNavigator.prototype, "gpu", {get: () => ({requestAdapter: async () => null})}); } catch {}',
  `function wModuleInit() { ${moduleCode.replace('var Module', 'var ___Module')}; return Module; }`,
  bundledString('LLAMA_CPP_WORKER_CODE'),
].join('\n');
const factory = /var createWorker = \(workerCode\) => \{[\s\S]*?\n\};/g;
if ([...source.matchAll(factory)].length !== 1) throw new Error('Unexpected upstream worker factory');
fs.mkdirSync(output, { recursive: true });
fs.writeFileSync(`${output}/firefox-worker.js`, worker);
fs.writeFileSync(`${output}/wllama.js`, source.replace(factory,
  'var createWorker = () => new Worker(new URL("./firefox-worker.js", import.meta.url), {type: "module"});'));
