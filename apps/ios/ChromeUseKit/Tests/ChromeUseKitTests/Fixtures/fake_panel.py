#!/usr/bin/env python3
"""A stand-in for the chrome-use panel service and the push Worker, for the
iOS end-to-end test and simulator screenshots (mirrors the fixture in
cli/tests/mobile_gateway.rs). It never touches ~/.chrome-use: everything
lives under --relay, which the gateway reads as CHROME_USE_RELAY_DIR.

stdout, one JSON object per line:
  {"ready":true,"push":"http://127.0.0.1:<port>"}   once listening
  {"push":{...}}                                      each push the gateway sends
  {"register":{...}}                                  each app registration (/v1/register)
stdin commands, one per line:
  request    the first conversation starts waiting for an approval
  reply      the agent answers with streamed text and ends the turn
"""
import argparse, hashlib, json, os, socket, struct, sys, threading, time, uuid
from http.server import BaseHTTPRequestHandler, HTTPServer

PROFILE = "mobile-fixture"
WEEKLY = "7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e"
INBOX = "3b0e9a52-6a3f-4f4e-9d0c-2f8f1a4b7c11"
DEPLOY = "c5a1f0d2-7e44-4b8a-a1c9-5e2b8d3f6a90"
SCHEDULE = "9f1c2d3e-4b5a-4c6d-8e7f-0a1b2c3d4e5f"

lock = threading.Lock()
now = lambda: int(time.time() * 1000)


def summary(c):
    return {"id": c["id"], "agentId": c["agentId"], "title": c["title"], "phase": c["phase"],
            "fullAccess": False, "options": c.get("options", {}), "scheduleId": c.get("scheduleId"),
            "lastTurn": c.get("lastTurn"), "updated": c["updated"], "created": c["updated"] - 60000,
            "control": "stopRequested" if c["phase"] == "stopping" else "agent", "model": c.get("model"),
            "queued": 0, "target": {"mode": "agentSession", "session": "panel-" + c["id"]}}


def ev(c, kind, data):
    c["events"].append({"sequence": len(c["events"]) + 1, "kind": kind, "data": data, "at": now()})
    c["updated"] = now()


def initial(demo):
    t = now()
    weekly = {"id": WEEKLY, "agentId": "claude", "title": "Weekly report", "phase": "idle", "updated": t - 5000, "events": []}
    convs = {WEEKLY: weekly}
    if demo:
        m = "1b4e28ba-2fa1-41d2-883f-0016d3cca427"
        ev(weekly, "user", {"messageId": m, "text": "整理本周的周报：打开 Jira 看看我这周关了哪些单"})
        ev(weekly, "turnStarted", {"messageId": m, "generation": 1})
        ev(weekly, "tool", {"messageId": m, "item": {"id": "t1", "tool": "mcp__chrome_use__chrome_use_open", "input": {"url": "https://jira.example.com/issues?filter=closed-this-week"}}, "completed": True})
        ev(weekly, "tool", {"messageId": m, "item": {"id": "t2", "tool": "mcp__chrome_use__chrome_use_read", "input": {"ref": "issue-table"}}, "completed": True})
        ev(weekly, "textFinal", {"messageId": m, "itemId": "a1", "part": 0, "text": "本周你关闭了 7 个单：\n• PAY-812 退款回调超时\n• PAY-815 汇率缓存\n• WEB-301 登录页闪烁\n……\n要我把它整理成周报草稿发到飞书吗？"})
        m2 = "6f1d2c3b-4a5e-4f60-8a7b-9c0d1e2f3a4b"
        ev(weekly, "user", {"messageId": m2, "text": "好，把提交记录也加上"})
        ev(weekly, "turnStarted", {"messageId": m2, "generation": 2})
        ev(weekly, "request", {"requestId": 7, "method": "can_use_tool", "generation": 2, "requestRevision": 1,
                               "params": {"tool_name": "Bash", "input": {"command": "git log --since=monday --author=leo --oneline"}}})
        weekly["phase"] = "waitingInput"
        inbox = {"id": INBOX, "agentId": "codex", "title": "Clean up my inbox", "phase": "running", "updated": t - 1000, "events": []}
        ev(inbox, "user", {"messageId": str(uuid.uuid4()), "text": "Clean up my inbox"})
        deploy = {"id": DEPLOY, "agentId": "claude", "title": "Check the deploy dashboard", "phase": "failed", "updated": t - 3600_000, "events": [],
                  "lastTurn": {"messageId": "x", "outcome": "failed", "ended": t - 3600_000, "preview": "Login wall on the dashboard"}}
        convs[INBOX] = inbox
        convs[DEPLOY] = deploy
    return convs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--relay", required=True)
    ap.add_argument("--binary", required=True)
    ap.add_argument("--demo", action="store_true")
    a = ap.parse_args()
    binary_hash = hashlib.sha256(open(a.binary, "rb").read()).hexdigest()
    state = {"convs": initial(a.demo), "schedules": [
        {"id": SCHEDULE, "name": "每日站会摘要", "prompt": "Summarize standup", "agentId": "claude", "rule": {"kind": "daily", "time": "09:30"},
         "enabled": True, "nextRun": now() + 3600_000, "lastResult": "completed", "lastChat": WEEKLY, "finishedAt": now() - 86400_000}]}
    out_lock = threading.Lock()

    def emit(obj):
        with out_lock:
            sys.stdout.write(json.dumps(obj) + "\n")
            sys.stdout.flush()

    def handle(op, p):
        convs = state["convs"]
        c = convs.get(p.get("conversationId")) if isinstance(p, dict) else None
        if op == "conversationList":
            return {"schemaVersion": 1, "conversations": [summary(x) for x in convs.values()]}
        if op == "scheduleList":
            return {"schemaVersion": 1, "schedules": state["schedules"]}
        if op == "scheduleRun":
            return {"schemaVersion": 1, "conversation": summary(convs[WEEKLY])}
        if op == "scheduleSave":
            for s in state["schedules"]:
                if s["id"] == p["id"]:
                    s["enabled"] = p["enabled"]
            return {"schemaVersion": 1, "schedules": state["schedules"]}
        if op == "conversationCreate":
            cid = str(uuid.uuid4())
            convs[cid] = {"id": cid, "agentId": p["agentId"], "title": "New conversation", "phase": "idle", "updated": now(), "events": [],
                          "options": p.get("options", {}), "model": p.get("model")}
            return {"schemaVersion": 1, "conversation": summary(convs[cid])}
        if c is None:
            return None
        if op in ("conversationSend", "conversationSendDirect"):
            c["phase"] = "running"
            if c["title"] == "New conversation":
                c["title"] = p["text"].splitlines()[0][:80]
            ev(c, "user", {"messageId": p["messageId"], "text": p["text"], "attachments": p.get("attachments", [])})
            return {"schemaVersion": 1, "conversation": summary(c), "message": {"id": p["messageId"], "accepted": True}}
        if op == "conversationRead":
            after = p.get("after", 0)
            events = [e for e in c["events"] if e["sequence"] > after]
            cursor = events[-1]["sequence"] if events else after
            return {"schemaVersion": 1, "conversation": summary(c), "journal": {"events": events, "cursor": cursor, "more": False}, "queuedMessages": []}
        if op == "conversationRespond":
            c["phase"] = "running"
            ev(c, "replySent", {"requestId": p["requestId"]})
            ev(c, "requestResolved", {"requestId": p["requestId"]})
            return {"schemaVersion": 1, "conversation": summary(c), "response": {"dispatched": True, "decision": p["decision"]}}
        if op == "conversationStop":
            c["phase"] = "idle"
            ev(c, "terminal", {"status": "interrupted"})
            return {"schemaVersion": 1, "conversation": summary(c)}
        if op == "conversationAttachmentPut":
            import base64
            raw = base64.b64decode(p["data"])
            return {"schemaVersion": 1, "conversation": summary(c), "attachment": {"id": hashlib.sha256(raw).hexdigest(), "mediaType": "image/jpeg", "bytes": len(raw)}}
        if op == "conversationDelete":
            del convs[c["id"]]
            return {"schemaVersion": 1, "deleted": True}
        return None

    def serve_panel(listener):
        while True:
            conn, _ = listener.accept()
            threading.Thread(target=one, args=(conn,), daemon=True).start()

    def one(conn):
        try:
            n = struct.unpack("<I", recv_exact(conn, 4))[0]
            req = json.loads(recv_exact(conn, n))
            assert req["capability"] == "c" * 64
            op, p = req["operation"], req.get("params", {})
            with lock:
                if op == "health":
                    reply = {"result": {"schemaVersion": 1, "binaryHash": binary_hash, "active": 0}}
                else:
                    result = handle(op, p)
                    reply = {"result": result} if result is not None else {"error": "unexpected operation in fixture: " + op}
            reply["requestId"] = req.get("requestId")
            raw = json.dumps(reply).encode()
            conn.sendall(struct.pack("<I", len(raw)) + raw)
        except Exception as e:  # noqa: BLE001
            sys.stderr.write("fake panel: %r\n" % e)
        finally:
            conn.close()

    def recv_exact(conn, n):
        buf = b""
        while len(buf) < n:
            chunk = conn.recv(n - len(buf))
            if not chunk:
                raise EOFError
            buf += chunk
        return buf

    class Push(BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("content-length", 0)))
            if self.path == "/v1/register":
                # The app's registration: a stand-in relay token.
                emit({"register": json.loads(body)})
                reply = json.dumps({"relayToken": "fixture." + uuid.uuid4().hex, "expiresAt": int(time.time()) + 90 * 86400}).encode()
            else:
                emit({"push": json.loads(body)})
                reply = b'{"status":"sent"}'
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(reply)))
            self.end_headers()
            self.wfile.write(reply)

        def log_message(self, *args):
            pass

    profile_dir = os.path.join(a.relay, "agent-panel", "profiles", PROFILE)
    for d in (a.relay, os.path.join(a.relay, "agent-panel"), os.path.join(a.relay, "agent-panel", "profiles"), profile_dir):
        os.makedirs(d, exist_ok=True)
        os.chmod(d, 0o700)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(32)
    endpoint = os.path.join(profile_dir, "service.json")
    with open(endpoint, "w") as f:
        json.dump({"schemaVersion": 1, "profile": PROFILE, "address": "127.0.0.1:%d" % listener.getsockname()[1],
                   "capability": "c" * 64, "binaryHash": binary_hash, "pid": os.getpid()}, f)
    os.chmod(endpoint, 0o600)
    threading.Thread(target=serve_panel, args=(listener,), daemon=True).start()
    push = HTTPServer(("127.0.0.1", 0), Push)
    threading.Thread(target=push.serve_forever, daemon=True).start()
    emit({"ready": True, "push": "http://127.0.0.1:%d" % push.server_address[1]})

    for line in sys.stdin:
        command = line.strip()
        with lock:
            c = state["convs"][WEEKLY]
            if command == "request":
                c["phase"] = "waitingInput"
                ev(c, "request", {"requestId": 7, "method": "can_use_tool", "generation": 1, "requestRevision": 3,
                                  "params": {"tool_name": "Bash", "input": {"command": "git log --since=monday --author=leo"}}})
            elif command == "codex":
                c["phase"] = "waitingInput"
                ev(c, "request", {"requestId": "r-9", "method": "item/commandExecution/requestApproval", "generation": 1, "requestRevision": 1,
                                  "params": {"command": "npm run deploy -- --env staging", "cwd": "/Users/leo/work/site", "reason": "Deploy to staging"}})
            elif command == "reply":
                m = c["events"][-1]["data"].get("messageId") if c["events"] else None
                ev(c, "textDelta", {"messageId": m, "itemId": "z", "text": "Done"})
                ev(c, "textFinal", {"messageId": m, "itemId": "z", "part": 0, "text": "Done."})
                ev(c, "terminal", {"messageId": m, "status": "completed"})
                c["phase"] = "idle"
        emit({"ack": command})


if __name__ == "__main__":
    main()
