#!/usr/bin/env python3
"""Phase 2 demo driver: proves the improvements against real headless Vivido.

Usage: python3 demo/improve.py {spellings,strict}

Like demo/drive.py, but every scenario exercises a Phase 2 behavior change:
forgiving spellings end to end (spellings), strict validation plus tiers plus
protocol hardening (strict). All output is real MCP output from the real
binary; screenshots report metadata only (never base64).
"""

import base64
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target", "debug", "vivido-mcp")
VIVIDO_BIN = os.environ.get("VIVIDO_BIN", "/tmp/vivido-upstream/target/release/vivido")

DIM = "\033[2m"
BOLD = "\033[1m"
GREEN = "\033[32m"
RED = "\033[31m"
CYAN = "\033[36m"
RESET = "\033[0m"


class Mcp:
    def __init__(self, *allow, target=None):
        args = [BIN]
        if target:
            args += ["--target", target]
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

    def request(self, method, params):
        rid = self.next_id
        self.next_id += 1
        self.proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}) + "\n"
        )
        self.proc.stdin.flush()
        return json.loads(self.proc.stdout.readline())

    def raw(self, frame):
        """Send one arbitrary JSON-RPC frame and read its reply (if any)."""
        self.proc.stdin.write(json.dumps(frame) + "\n")
        self.proc.stdin.flush()
        return json.loads(self.proc.stdout.readline())

    def call(self, name, args):
        reply = self.request("tools/call", {"name": name, "arguments": args})
        result = reply.get("result", {})
        return result.get("content", []), result.get("structuredContent", {}), result.get(
            "isError", False
        )

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def say(text):
    print(f"\n{DIM}── {text} ──{RESET}")


def short(value, width=88):
    text = value if isinstance(value, str) else json.dumps(value)
    return text if len(text) <= width else text[: width - 1] + "…"


def run(mcp, name, args):
    shown = " ".join(f"{k}={json.dumps(v)}" for k, v in args.items())
    print(f"{BOLD}$ {name}{RESET} {DIM}{short(shown, 80)}{RESET}")
    time.sleep(0.25)
    content, structured, is_error = mcp.call(name, args)
    if is_error:
        error = structured.get("error", {})
        print(f"  {RED}✗ {error.get('code')}: {short(error.get('message', ''), 200)}{RESET}")
    else:
        print(f"  {GREEN}→{RESET} {short(summarize(name, content, structured), 90)}")
    time.sleep(0.55)
    return content, structured, is_error


def summarize(name, content, structured):
    if name == "list_windows":
        windows = structured.get("windows", [])
        parts = [
            f"#{w.get('window_id')} {w.get('grid', {}).get('columns')}x{w.get('grid', {}).get('rows')}"
            for w in windows
        ]
        return f"{len(windows)} window(s): " + ", ".join(parts)
    if name in ("screenshot", "capture"):
        image = next((b for b in content if b.get("type") == "image"), None)
        if image is None:
            return "no image block?!"
        raw = base64.b64decode(image["data"])
        return (
            f"{structured.get('mime')} {structured.get('width')}x{structured.get('height')} "
            f"{len(raw) // 1024}KB encoded={structured.get('encoded_bytes')}B"
        )
    if name in ("typing", "type_text"):
        return f"wrote {structured.get('written_bytes')} bytes"
    if name == "key":
        return "keypress delivered"
    if name == "signal":
        return "signal delivered"
    if name == "mouse":
        return "mouse action delivered"
    if name.startswith("wait_"):
        return f"matched: {short(structured, 70)}"
    if name == "get_text":
        lines = [line for line in structured.get("text", "").splitlines() if line.strip()]
        tail = lines[-2:] if lines else []
        return "screen shows: " + " / ".join(repr(line.strip()) for line in tail)
    if name == "headless_start":
        return f"session {structured.get('session')!r} started and attached"
    if name == "headless_stop":
        return f"session {structured.get('session')!r} stopped via {structured.get('method')}"
    return short(structured, 80)


def wait_shell(mcp, timeout=12):
    deadline = time.time() + timeout
    while time.time() < deadline:
        _, structured, is_error = mcp.call("get_text", {})
        if not is_error and "$" in structured.get("text", ""):
            return True
        time.sleep(0.5)
    return False


def output_offset(mcp):
    _, insp, failed = mcp.call("inspect", {"window_id": 1})
    if failed:
        return None
    return insp.get("window", {}).get("sequences", {}).get("output")


def scenario_spellings():
    mcp = Mcp("process")
    say("fresh ephemeral session (real headless Vivido)")
    _, started, failed = mcp.call("headless_start", {"size": "1280x720px"})
    session = started.get("session", "?")
    print(f"  {GREEN}→{RESET} session {session!r} started and attached")
    time.sleep(0.55)
    if failed:
        mcp.close()
        return
    wait_shell(mcp)

    say("forgiving spellings, real effects")
    base = output_offset(mcp)
    run(mcp, "typing", {"text": "echo spell-$((20+22))\n"})
    run(mcp, "key", {"key": "Return"})  # alias of Enter
    run(mcp, "wait_output", {"pattern": "spell-42", "after_offset": base, "timeout": "10S"})
    run(mcp, "signal", {"signal": "SIGWINCH"})  # SIG- prefix, harmless
    run(mcp, "mouse", {"action": "Click", "cell_column": 4, "cell_row": 2})
    run(mcp, "key", {"key": "Up"})  # alias of ArrowUp

    say("verify: text and a settled JPEG capture")
    run(mcp, "get_text", {})
    run(mcp, "capture", {"stable": True, "format": "jpeg", "max_dim": 960})
    run(mcp, "headless_stop", {"target": session})
    mcp.close()


def scenario_strict():
    say("CLI fails fast, with a suggestion")
    proc = subprocess.run(
        [BIN, "--allow", "inputs"], capture_output=True, text=True, timeout=10
    )
    print(f"{BOLD}$ vivido-mcp --allow inputs{RESET}")
    time.sleep(0.25)
    print(f"  {RED}✗ exit {proc.returncode}: {short(proc.stderr.strip().splitlines()[-1], 84)}{RESET}")
    time.sleep(0.55)

    say("fresh ephemeral session for the battery")
    admin = Mcp("process")
    _, started, failed = admin.call("headless_start", {"size": "1280x720px"})
    session = started.get("session", "?")
    print(f"  {GREEN}→{RESET} session {session!r} started and attached")
    time.sleep(0.55)
    if failed:
        admin.close()
        return
    wait_shell(admin)

    say("observe-only: refusal names the env fix, typos get prefixes")
    obs = Mcp(target=session)
    run(obs, "key", {"key": "Enter"})
    run(obs, "headless", {})
    obs.close()

    say("strict types fail fast with invalid_args")
    mcp = Mcp("process", target=session)
    run(mcp, "drain_events", {"events": ["bell", 42], "timeout": "100ms"})
    run(mcp, "capture", {"activate": "yes"})
    run(mcp, "headless_start", {"session": 42})
    run(mcp, "headless_start", {"size": "99999x40"})

    say("protocol layer: prompts and JSON-RPC shape")
    reply = mcp.request(
        "prompts/get",
        {"name": "vivido_workflow", "arguments": {"goal": "x", "bogus": 1}},
    )
    print(f"{BOLD}$ prompts/get{RESET} {DIM}goal=\"x\" bogus=1{RESET}")
    time.sleep(0.25)
    print(f"  {RED}✗ -32602: {short(reply['error']['message'], 78)}{RESET}")
    time.sleep(0.55)
    reply = mcp.raw({"id": 9001, "method": "ping", "params": {}})
    print(f"{BOLD}$ ping{RESET} {DIM}(frame without jsonrpc){RESET}")
    time.sleep(0.25)
    print(f"  {RED}✗ {reply['error']['code']}: {short(reply['error']['message'], 76)}{RESET}")
    time.sleep(0.55)

    say("capture honors format + reports raw bytes")
    run(mcp, "capture", {"format": "jpeg", "max_dim": 960})
    run(mcp, "headless_stop", {"target": session})
    mcp.close()
    admin.close()


SCENARIOS = {"spellings": scenario_spellings, "strict": scenario_strict}

if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else ""
    if which not in SCENARIOS:
        sys.exit(f"usage: improve.py {{{'|'.join(SCENARIOS)}}}")
    print(f"{BOLD}vivido-mcp improvements: {which}{RESET} {DIM}(real headless Vivido){RESET}")
    SCENARIOS[which]()
    print(f"\n{DIM}done.{RESET}")
