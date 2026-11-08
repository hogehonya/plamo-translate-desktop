import json
import time
from pathlib import Path
from selenium import webdriver
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service
import argparse

parser = argparse.ArgumentParser(
    description="Firefox integration test; downloads the 573 MB PLaMo model into an isolated profile."
)
parser.add_argument(
    "--accept-model-license",
    action="store_true",
    help="Confirm you accept the PLaMo Community License for this test download",
)
args = parser.parse_args()
if not args.accept_model_license:
    parser.error(
        "Read the model license linked in docs/firefox-android.md, then pass --accept-model-license"
    )
root = Path(__file__).resolve().parents[2]
opts = Options()
opts.add_argument("-headless")
opts.set_preference("intl.locale.requested", "en-US")
opts.set_preference(
    "extensions.webextensions.uuids",
    json.dumps(
        {"plamo-translate@hogehonya.github.io": "83ade946-7767-4ccb-bbc1-668ddcd8a118"}
    ),
)
d = webdriver.Firefox(
    options=opts, service=Service(service_args=["--allow-system-access"])
)
try:
    print("Firefox", d.capabilities["browserVersion"], flush=True)
    d.install_addon(
        str(next((root / "dist").glob("*-firefox-unsigned.zip"))), temporary=True
    )
    d.set_context("chrome")
    d.execute_script(
        "Services.wm.getMostRecentWindow('navigator:browser').gBrowser.selectedBrowser.loadURI(Services.io.newURI('moz-extension://83ade946-7767-4ccb-bbc1-668ddcd8a118/panel.html'), {triggeringPrincipal: Services.scriptSecurityManager.getSystemPrincipal()})"
    )
    d.set_context("content")
    time.sleep(4)
    assert "No model downloaded" in d.find_element("id", "status").text
    print(
        "FEATURES",
        d.execute_script(
            "return {sab:typeof SharedArrayBuffer,isolated:crossOriginIsolated,opfs:typeof navigator.storage.getDirectory,gpu:typeof navigator.gpu};"
        ),
        flush=True,
    )
    print(
        "OPFS",
        d.execute_async_script(
            'const done=arguments[0]; navigator.storage.getDirectory().then(async d=>{const f=await d.getFileHandle("smoke",{create:true});const w=await f.createWritable();await w.write("ok");await w.close();const result=await (await f.getFile()).text();await d.removeEntry("smoke"); done(result);}).catch(e=>done(String(e)));'
        ),
        flush=True,
    )
    d.find_element("css selector", "#settings summary").click()
    rows = d.find_elements("css selector", ".model-row")
    print("MODELS", [row.text for row in rows], flush=True)
    if "Delete" not in rows[0].text:
        rows[0].find_element("css selector", "button").click()
        d.find_element("id", "licenseAccept").click()
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            row = d.execute_script(
                "const row=document.querySelector('.model-row');return {text:row.innerText,error:!!row.querySelector('.error')};"
            )
            print("DOWNLOAD", row["text"], flush=True)
            if "Delete" in row["text"]:
                break
            if row["error"]:
                raise RuntimeError(row["text"])
            time.sleep(10)
        else:
            raise RuntimeError("download timeout")
    d.find_element("css selector", "#settings summary").click()
    from selenium.webdriver.support.ui import Select

    Select(d.find_element("id", "target")).select_by_value("ja")
    for trial in range(2):
        d.find_element("id", "source").clear()
        d.find_element("id", "source").send_keys("The sky is blue.")
        d.find_element("id", "translate").click()
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            time.sleep(3)
            status = d.find_element("id", "status").text
            output = d.find_element("id", "output").text
            notice = d.find_element("id", "notice").text
            print("TRANSLATE", trial, status, output, notice, flush=True)
            if notice:
                raise RuntimeError(notice)
            if output and d.find_element("id", "translate").text == "Translate":
                break
        else:
            raise RuntimeError("translation timeout")
        assert any(ord(c) > 255 for c in output), output
        d.set_window_size(390, 844)
        d.save_screenshot("/tmp/plamo-firefox-translated.png")
        d.find_element("id", "release").click()
        time.sleep(2)
    # Verify the panel targets the source page, not itself. Serve only this fixture.
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    import threading

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(
                b'<!doctype html><html lang="en"><body><p id="fixture">The sky is blue. The sun is bright today.</p></body></html>'
            )

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    d.set_script_timeout(20)
    source = d.execute_async_script(
        "const done=arguments[arguments.length-1]; browser.tabs.create({url:arguments[0],active:false}).then(t=>done(t.id));",
        f"http://127.0.0.1:{server.server_port}/",
    )
    panel_handle = d.current_window_handle
    # tabs.create resolves before navigation completes; granting activeTab on
    # about:blank would immediately be revoked when the fixture loads.
    time.sleep(2)
    for handle in d.window_handles:
        d.switch_to.window(handle)
        if d.current_url.startswith("http://127.0.0.1:"):
            assert d.find_element("id", "fixture").text
            break
    else:
        raise RuntimeError("source fixture did not load")
    d.switch_to.window(panel_handle)
    # Grant the same temporary source-tab permission that a toolbar click grants.
    d.set_context("chrome")
    d.execute_script(
        "const e=WebExtensionPolicy.getByID('plamo-translate@hogehonya.github.io').extension; const tab=e.tabManager.get(arguments[0]); e.tabManager.addActiveTabPermission(tab.nativeTab);",
        source,
    )
    d.set_context("content")
    d.execute_async_script(
        "const done=arguments[arguments.length-1]; browser.runtime.getBackgroundPage().then(bg=>bg.openPanel({id:arguments[0]})).then(()=>done(true)).catch(e=>done(String(e)));",
        source,
    )
    time.sleep(3)
    d.switch_to.window(d.window_handles[-1])
    print("PAGE_PANEL", d.current_url, flush=True)
    assert f"sourceTab={source}" in d.current_url
    d.find_element("id", "translatePage").click()
    time.sleep(1)
    notice = d.find_element("id", "notice").text
    if notice:
        raise RuntimeError(f"Page injection failed: {notice}")
    d.execute_async_script(
        "const done=arguments[arguments.length-1];browser.tabs.update(arguments[0],{active:true}).then(()=>done(true));",
        source,
    )
    time.sleep(2)
    # Locate the source tab by URL, independent of tab handle ordering.
    for handle in d.window_handles:
        d.switch_to.window(handle)
        if d.current_url.startswith("http://127.0.0.1:"):
            break
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        text = d.find_element("id", "fixture").text
        print("PAGE_TEXT", text, flush=True)
        if any(ord(c) > 255 for c in text):
            break
        time.sleep(3)
    else:
        raise RuntimeError("page translation timeout")
    server.shutdown()
    print(
        "PASS: download, translation, release/reload and original-page translation",
        flush=True,
    )
finally:
    d.quit()
