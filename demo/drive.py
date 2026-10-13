#!/usr/bin/env python3
"""Demo driver: speaks MCP to a real vivido-mcp + headless Vivido, narrates.

Usage: python3 demo/drive.py {observe,act,tiers,headless}

Output is paced, colored, and compact for VHS recording. Screenshots are saved
under assets/ and only their metadata is shown (never base64).
"""

import base64
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = [os.path.join(ROOT, "target", "debug", "vivido-mcp")]
VIVIDO_BIN = "/tmp/vivido-upstream/target/release/vivido"
TARGET = "demodrive"
ASSETS = os.path.join(ROOT, "assets")

DIM = "\033[2m"
BOLD = "\033[1m"
GREEN = "\033[32m"
RED = "\033[31m"
CYAN = "\033[36m"
RESET = "\033[0m"


class Mcp:
    def __init__(self, *allow):
        args = BIN + ["--target", TARGET]
        for tier in allow:
            args += ["--allow", tier]
        env = dict(os.environ)
        env["VIVIDO_BIN"] = VIVIDO_BIN
        self.proc = subprocess.Popen(
            args,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            env=env,
        )
        self.next_id = 1
        self.allow = allow

    def request(self, method, params):
        rid = self.next_id
        self.next_id += 1
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}) + "\n")
        self.proc.stdin.flush()
        return json.loads(self.proc.stdout.readline())

    def call(self, name, args):
        reply = self.request("tools/call", {"name": name, "arguments": args})
        result = reply.get("result", {})
        return result.get("content", []), result.get("structuredContent", {}), result.get("isError", False)

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def say(text):
    print(f"\n{DIM}── {text} ──{RESET}")
    time.sleep(0.5)


def short(value, width=88):
    text = value if isinstance(value, str) else json.dumps(value)
    return text if len(text) <= width else text[: width - 1] + "…"


def run(mcp, name, args, save_as=None):
    shown = " ".join(f"{k}={json.dumps(v)}" for k, v in args.items())
    print(f"{BOLD}$ {name}{RESET} {DIM}{short(shown, 80)}{RESET}")
    time.sleep(0.4)
    content, structured, is_error = mcp.call(name, args)
    if is_error:
        error = structured.get("error", {})
        print(f"  {RED}✗ {error.get('code')}: {short(error.get('message', ''), 84)}{RESET}")
    else:
        print(f"  {GREEN}→{RESET} {short(summarize(name, content, structured, save_as), 90)}")
    time.sleep(0.9)
    return content, structured, is_error


def summarize(name, content, structured, save_as):
    if name == "list_windows":
        windows = structured.get("windows", [])
        parts = [f"#{w.get('window_id')} {w.get('title')!r} {w.get('grid', {}).get('columns')}x{w.get('grid', {}).get('rows')}" for w in windows]
        return f"{len(windows)} window(s): " + ", ".join(parts)
    if name == "inspect":
        seq = structured.get("window", {}).get("sequences", {})
        return f"screen={seq.get('screen')} frame={seq.get('frame')} output={seq.get('output')}"
    if name in ("screenshot", "capture"):
        image = next((b for b in content if b.get("type") == "image"), None)
        if image is None:
            return "no image block?!"
        raw = base64.b64decode(image["data"])
        if save_as:
            os.makedirs(ASSETS, exist_ok=True)
            with open(os.path.join(ASSETS, save_as), "wb") as f:
                f.write(raw)
        return f"{structured.get('mime')} {structured.get('width')}x{structured.get('height')} {len(raw)//1024}KB → assets/{save_as}"
    if name in ("typing", "type_text"):
        return f"wrote {structured.get('written_bytes')} bytes to window {structured.get('window_id')}"
    if name == "key":
        return "keypress delivered"
    if name.startswith("wait_"):
        return f"matched: {short(structured, 70)}"
    if name == "get_text":
        lines = [line for line in structured.get("text", "").splitlines() if line.strip()]
        tail = lines[-2:] if lines else []
        return "screen shows: " + " / ".join(repr(line.strip()) for line in tail)
    if name == "headless_list":
        names = [f"{s['name']}{'' if s['live'] else ' (dead)'}" for s in structured.get("sessions", [])]
        return f"sessions: {', '.join(names) or '(none)'}"
    if name == "headless_start":
        return f"session {structured.get('session')!r} started and attached"
    if name == "headless_stop":
        return f"session {structured.get('session')!r} stopped via {structured.get('method')}"
    return short(structured, 80)


def wait_shell(mcp, timeout=15):
    """Wait until the demo shell shows a prompt."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        _, structured, is_error = mcp.call("get_text", {})
        if not is_error and "$" in structured.get("text", ""):
            return True
        time.sleep(0.5)
    return False


def scenario_observe():
    mcp = Mcp()
    say("connect: tools generated from the live hello")
    reply = mcp.request("initialize", {})
    info = reply["result"]["serverInfo"]
    print(f"  {GREEN}→{RESET} {info['name']} {info['version']}")
    time.sleep(0.6)
    reply = mcp.request("tools/list", {})
    names = [t["name"] for t in reply["result"]["tools"]]
    print(f"  {GREEN}→{RESET} {len(names)} tools (screenshot, capture, drain_events, …)")
    time.sleep(0.9)
    say("discover the terminal")
    run(mcp, "list_windows", {})
    run(mcp, "inspect", {"window_id": 1})
    say("read it two ways: text and pixels")
    run(mcp, "get_text", {})
    run(mcp, "screenshot", {"max_dim": 960}, save_as="shot-observe.png")
    mcp.close()


def output_offset(mcp):
    _, insp, failed = mcp.call("inspect", {"window_id": 1})
    if failed:
        return None
    return insp.get("window", {}).get("sequences", {}).get("output")


def type_and_wait(mcp, text, pattern, timeout="15s"):
    """Type a command and wait for its output, anchoring on the pre-act offset
    so the wait can never be satisfied by — or miss — past bytes."""
    base = output_offset(mcp)
    _, _, failed = run(mcp, "typing", {"text": text})
    if failed or base is None:
        return
    args = {"pattern": pattern, "timeout": timeout}
    if base is not None:
        args["after_offset"] = base
    _, _, failed = run(mcp, "wait_output", args)
    if failed:  # one retry with a fresh offset
        base = output_offset(mcp)
        run(mcp, "typing", {"text": text})
        args["after_offset"] = base
        run(mcp, "wait_output", args)


def scenario_act():
    mcp = Mcp("input")
    say("wait for a ready shell, then act")
    wait_shell(mcp)
    print(f"  {GREEN}→{RESET} prompt is ready")
    time.sleep(0.5)
    type_and_wait(mcp, "echo demo-$((40+2))\n", "demo-42")
    say("verify with a settled capture")
    run(mcp, "capture", {"stable": True, "max_dim": 960}, save_as="shot-act.png")
    mcp.close()


def scenario_tiers():
    say("default server: observe-only")
    mcp = Mcp()
    run(mcp, "list_windows", {})
    run(mcp, "key", {"key": "Enter"})
    mcp.close()
    time.sleep(0.6)
    say("restart with --allow input")
    print(f"  {CYAN}$ vivido-mcp --allow input{RESET}")
    time.sleep(0.8)
    mcp = Mcp("input")
    run(mcp, "key", {"key": "Enter"})
    run(mcp, "quit", {})
    mcp.close()


def scenario_headless():
    mcp = Mcp("process")
    say("list sessions, then start an ephemeral one")
    run(mcp, "headless_list", {})
    _, started, failed = mcp.call(
        "headless_start", {"session": "democlip", "size": "1280x720px"}
    )
    print(f"  {GREEN}→{RESET} session 'democlip' started and attached" if not failed else f"  {RED}✗ {started}{RESET}")
    time.sleep(0.9)
    if not failed:
        say("it is a real terminal: type, wait, capture")
        wait_shell(mcp)
        type_and_wait(mcp, "echo clip-$((20+22))\n", "clip-42")
        run(mcp, "capture", {"stable": True, "max_dim": 960}, save_as="shot-headless.png")
        say("stop it when done")
        run(mcp, "headless_stop", {"target": "democlip"})
    run(mcp, "headless_list", {})
    mcp.close()


SCENARIOS = {
    "observe": scenario_observe,
    "act": scenario_act,
    "tiers": scenario_tiers,
    "headless": scenario_headless,
}

if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else ""
    if which not in SCENARIOS:
        sys.exit(f"usage: drive.py {{{'|'.join(SCENARIOS)}}}")
    print(f"{BOLD}vivido-mcp demo: {which}{RESET} {DIM}(real headless Vivido behind MCP){RESET}")
    SCENARIOS[which]()
    print(f"\n{DIM}done.{RESET}")
