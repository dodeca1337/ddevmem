#!/usr/bin/env python3
"""Regenerate the web-UI screenshots used by README.md.

Drives headless Chrome over the DevTools Protocol rather than using
`--screenshot`, because the shots need things the CLI flag cannot do:
a 2x pixel density, `prefers-color-scheme` emulation for the light/dark
pair, waiting until the register values have actually been fetched, and
running page JS to expand a sidebar group before the frame is taken.

Usage:
    cargo run --example web_showcase --features web    # in another shell
    python3 docs/capture-screenshots.py

Requires: google-chrome-stable (or set $CHROME) and `websocket-client`
(pip install websocket-client).
"""
import base64
import json
import os
import subprocess
import sys
import time
import urllib.request

import websocket

URL = os.environ.get("DDEVMEM_URL", "http://localhost:8800/hw")
CHROME = os.environ.get("CHROME", "google-chrome-stable")
OUT_DIR = os.path.dirname(os.path.abspath(__file__))
PORT = 9222
SCALE = 2


class Chrome:
    """A headless Chrome instance with an open DevTools session."""

    def __init__(self):
        self.proc = subprocess.Popen(
            [
                CHROME,
                "--headless=new",
                f"--remote-debugging-port={PORT}",
                "--remote-allow-origins=*",
                "--disable-gpu",
                "--no-sandbox",
                "--hide-scrollbars",
                "about:blank",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        ws_url = None
        for _ in range(50):
            try:
                targets = json.load(
                    urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list")
                )
                pages = [t for t in targets if t["type"] == "page"]
                if pages:
                    ws_url = pages[0]["webSocketDebuggerUrl"]
                    break
            except Exception:
                pass
            time.sleep(0.2)
        if not ws_url:
            raise RuntimeError("could not attach to Chrome")
        self.ws = websocket.create_connection(ws_url, timeout=30, suppress_origin=True)
        self.id = 0
        self.send("Page.enable")
        self.send("Runtime.enable")

    def send(self, method, **params):
        self.id += 1
        self.ws.send(json.dumps({"id": self.id, "method": method, "params": params}))
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == self.id:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {})

    def eval(self, expression):
        result = self.send(
            "Runtime.evaluate",
            expression=expression,
            returnByValue=True,
            awaitPromise=True,
        )
        return result.get("result", {}).get("value")

    def wait_for(self, expression, what, tries=100):
        for _ in range(tries):
            if self.eval(expression):
                return
            time.sleep(0.1)
        raise RuntimeError(f"timed out waiting for {what}")

    def close(self):
        self.ws.close()
        self.proc.terminate()
        self.proc.wait(timeout=10)


def shot(chrome, name, width, height, theme, setup_js="", clip_selector=None):
    chrome.send(
        "Emulation.setDeviceMetricsOverride",
        width=width,
        height=height,
        deviceScaleFactor=SCALE,
        mobile=False,
    )
    chrome.send(
        "Emulation.setEmulatedMedia",
        features=[{"name": "prefers-color-scheme", "value": theme}],
    )
    # about:blank first so the page's inline theme bootstrap re-runs against
    # the emulated colour scheme.
    chrome.send("Page.navigate", url="about:blank")
    time.sleep(0.2)
    chrome.send("Page.navigate", url=URL)
    chrome.wait_for("typeof allMaps !== 'undefined' && allMaps.length > 0", "map list")
    chrome.wait_for(
        "(document.querySelector('.hex')||{}).textContent?.startsWith('0x')",
        "register values",
    )
    chrome.eval(
        "document.documentElement.setAttribute('data-theme', '%s'); updateThemeButton();"
        % ("g10" if theme == "light" else "g100")
    )
    if setup_js:
        chrome.eval(setup_js)
    time.sleep(0.4)

    # Viewport-sized by default: a full-page capture of the showcase is an
    # 8000 px strip. `captureBeyondViewport` is only needed when clipping to
    # an element that may sit below the fold.
    params = {"format": "png", "captureBeyondViewport": bool(clip_selector)}
    if clip_selector:
        box = chrome.eval(
            "(() => { const e = document.querySelector(%r); if (!e) return null;"
            " const r = e.getBoundingClientRect();"
            " return {x: r.x + scrollX, y: r.y + scrollY,"
            " width: r.width, height: r.height}; })()" % clip_selector
        )
        if not box:
            raise RuntimeError(f"selector not found: {clip_selector}")
        pad = 12
        params["clip"] = {
            "x": max(0, box["x"] - pad),
            "y": max(0, box["y"] - pad),
            "width": box["width"] + pad * 2,
            "height": box["height"] + pad * 2,
            "scale": 1,
        }

    path = os.path.join(OUT_DIR, name)
    with open(path, "wb") as f:
        f.write(base64.b64decode(chrome.send("Page.captureScreenshot", **params)["data"]))
    print(f"{name:26s} {os.path.getsize(path) // 1024:4d} KB")


def main():
    chrome = Chrome()
    try:
        # Hero: top of the page with the UART group expanded in the sidebar.
        expand_uart = "toggleNavGroup('uart', true); scrollTo(0, 0);"
        shot(chrome, "ui-dark.png", 1440, 900, "dark", expand_uart)
        shot(chrome, "ui-light.png", 1440, 900, "light", expand_uart)

        # Close-up of the UART control register: typed bitfields and docs.
        for theme in ("dark", "light"):
            shot(
                chrome,
                f"ui-register-{theme}.png",
                1280,
                900,
                theme,
                clip_selector="#reg-uart-0",
            )

        # Field-level access: the UART interrupt register, where w1c flags
        # share a word with read-write configuration.
        for theme in ("dark", "light"):
            shot(
                chrome,
                f"ui-mixed-{theme}.png",
                1440,
                760,
                theme,
                "document.getElementById('reg-uart-12')"
                ".scrollIntoView({block: 'start'});",
            )

        # Register arrays: fifo[0..8] / chan[0..4] expanded in the sidebar.
        for theme in ("dark", "light"):
            shot(
                chrome,
                f"ui-arrays-{theme}.png",
                1440,
                900,
                theme,
                "toggleNavGroup('dma', true);"
                "document.getElementById('map-dma').scrollIntoView();",
            )
    finally:
        chrome.close()


if __name__ == "__main__":
    sys.exit(main())
