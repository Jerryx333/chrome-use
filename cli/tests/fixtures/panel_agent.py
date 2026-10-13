#!/usr/bin/env python3
"""Synthetic CLI protocol fixture. Never starts a browser, shell tool or model."""
import json
import os
import sys
import time

if "--version" in sys.argv:
    print("0.0.1 (Claude Code)"); sys.exit(0)

if "models" in sys.argv:
    print("synthetic/opencode-model"); sys.exit(0)
acp = "acp" in sys.argv
codex = "app-server" in sys.argv
inspection = "--no-session-persistence" in sys.argv
session = str(__import__("uuid").uuid4()) if os.environ.get("PANEL_FIXTURE_UUID_SESSION") else ("synthetic-codex-thread" if codex else "synthetic-claude-session")
turn = None
pending = None
prompt_id = None
acp_stopping = False


def emit(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)


def complete(text, status="completed"):
    global turn
    if acp:
        emit({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}})
        emit({"jsonrpc":"2.0","id":prompt_id,"result":{"stopReason":{"completed":"end_turn","interrupted":"cancelled","failed":"max_tokens"}[status]}})
    elif codex:
        emit({"method": "item/agentMessage/delta", "params": {"threadId": session, "turnId": turn, "itemId": "fixture-output", "delta": text}})
        emit({"method": "turn/completed", "params": {"threadId": session, "turn": {"id": turn, "status": status, "error": {"message": "synthetic failure"} if status == "failed" else None}}})
    else:
        emit({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": text}}})
        emit({"type": "result", "subtype": "success" if status == "completed" else "error_during_execution", "is_error": status != "completed", "session_id": session})
    turn = None


for line in sys.stdin:
    message = json.loads(line)
    if acp:
        method = message.get("method")
        params = message.get("params", {})
        if method == "initialize":
            assert params["protocolVersion"] == 1
            assert params["clientCapabilities"]["terminal"] is False
            emit({"jsonrpc":"2.0","id":message["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":True,"sessionCapabilities":{"resume":{},"delete":{}}}}})
        elif method in ("session/new", "session/resume", "session/load"):
            inspection = not params["mcpServers"]
            env = {row["name"]:row["value"] for row in params["mcpServers"][0]["env"]} if not inspection else {"AGENT_BROWSER_BROWSER":"fixture","AGENT_BROWSER_SESSION":"panel-fixture","CHROME_USE_RELAY_DIR":os.environ["CHROME_USE_RELAY_DIR"]}
            assert env["AGENT_BROWSER_BROWSER"]
            assert env["AGENT_BROWSER_SESSION"].startswith("panel-")
            assert env["CHROME_USE_RELAY_DIR"] == os.environ["CHROME_USE_RELAY_DIR"]
            if method != "session/new":
                session = params["sessionId"]
                # A noisy backend must not duplicate retained history on resume.
                emit({"method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"REPLAY MUST NOT APPEAR"}}}})
            emit({"jsonrpc":"2.0","id":message["id"],"result":{"sessionId":session,"configOptions":[{"id":"model","type":"select","currentValue":"synthetic/opencode-model","options":[{"value":"synthetic/opencode-model","name":"Synthetic OpenCode"}]}]}})
        elif method == "session/set_config_option":
            assert params["configId"] == "model"
            emit({"jsonrpc":"2.0","id":message["id"],"result":{"configOptions":[{"id":"model","currentValue":params["value"]}]}})
        elif method == "session/delete":
            assert params["sessionId"]
            with open(os.path.join(os.environ["CHROME_USE_RELAY_DIR"],"acp-deletions.log"),"a") as proof: proof.write(params["sessionId"]+"\n")
            emit({"jsonrpc":"2.0","id":message["id"],"result":{}})
        elif method == "session/prompt":
            prompt_id = message["id"]
            turn = str(prompt_id)
            text = params["prompt"][0]["text"].split("\n\n")[-1]
            if text == "hello":
                assert 'chrome-tab:74' in params["prompt"][0]["text"]
                assert 'Synthetic ACP page' in params["prompt"][0]["text"]
            update = {"sessionUpdate":"tool_call","toolCallId":"fixture-acp-tool","title":"Synthetic tool","status":"in_progress","rawInput":{"command":"synthetic"}}
            emit({"method":"session/update","params":{"sessionId":session,"update":update}})
            update = {"toolCallId":"fixture-acp-tool","status":"completed","sessionUpdate":"tool_call_update"}
            emit({"method":"session/update","params":{"sessionId":session,"update":update}})
            if text in ("wait","wait-exit"): continue
            if text == "approval":
                pending = "fixture-acp-permission"
                emit({"jsonrpc":"2.0","id":pending,"method":"session/request_permission","params":{"sessionId":session,"toolCall":{"title":"Synthetic command","rawInput":{"command":"synthetic"}},"options":[{"optionId":"once","name":"Allow once","kind":"allow_once"},{"optionId":"deny","name":"Reject","kind":"reject_once"}]}})
                continue
            complete("synthetic reply: " + text,"failed" if text == "fail" else "completed")
        elif method == "session/cancel":
            if text == "wait-exit": sys.exit(0)
            if pending is not None:
                acp_stopping = True
                continue
            complete("synthetic stop","interrupted")
        elif method is None and message.get("id") == pending:
            assert message["result"]["outcome"]["outcome"] in ("selected","cancelled")
            if acp_stopping:
                assert message["result"]["outcome"]["outcome"] == "cancelled"
                complete("synthetic stop","interrupted")
                acp_stopping = False
            else:
                complete("synthetic approval handled")
            pending = None
    elif codex:
        method = message.get("method")
        if method == "initialize":
            inspection = message["params"]["clientInfo"]["name"] == "chrome_use_inspection"
            emit({"id": message["id"], "result": {}})
        elif method == "account/read":
            assert message["params"]["refreshToken"] is False
            emit({"id":message["id"],"result":{"account":{"type":"chatgpt","email":"private@synthetic.invalid","accessToken":"synthetic-never-transport"},"requiresOpenaiAuth":True}})
        elif method == "model/list":
            page = message["params"].get("cursor")
            data=[{"model":"synthetic-codex-model","displayName":"Synthetic Codex","isDefault":True,"supportedReasoningEfforts":[{"reasoningEffort":"high"}]}] if page is None else [{"model":"synthetic-codex-alternate","displayName":"Synthetic alternate","isDefault":False}]
            emit({"id":message["id"],"result":{"data":data,"nextCursor":"page-two" if page is None else None}})
        elif method == "config/read":
            emit({"id": message["id"], "result": {"config": {"mcp_servers": {"other_browser": {"command": "never-executed"}}}}})
        elif method in ("thread/start", "thread/resume"):
            assert not inspection, "Metadata inspection started a conversation"
            params = message["params"]
            mcp = params["config"]["mcp_servers"]
            assert mcp["other_browser"]["enabled"] is False
            assert mcp["chrome_use"]["env"]["AGENT_BROWSER_BROWSER"]
            assert mcp["chrome_use"]["env"]["AGENT_BROWSER_SESSION"].startswith("panel-")
            assert mcp["chrome_use"]["env"]["CHROME_USE_RELAY_DIR"] == os.environ["CHROME_USE_RELAY_DIR"]
            if method == "thread/resume":
                assert params["threadId"] == session
            policy = f'{params.get("approvalPolicy")}/{params.get("sandbox")}'
            emit({"id": message["id"], "result": {"thread": {"id": session}, "model": "synthetic-codex-model"}})
        elif method == "turn/start":
            assert not inspection, "Metadata inspection sent a prompt"
            turn = "turn-" + message["params"]["clientUserMessageId"]
            text = message["params"]["input"][0]["text"]
            emit({"id": message["id"], "result": {"turn": {"id": turn}}})
            emit({"method": "turn/started", "params": {"threadId": session, "turn": {"id": turn}}})
            if text in ("wait", "wait-exit"):
                continue
            if text == "approval":
                pending = 17
                emit({"id": pending, "method": "item/commandExecution/requestApproval", "params": {"threadId": session, "turnId": turn, "itemId": "fixture-tool", "command": "synthetic command, never executed"}})
                continue
            if text.endswith("echo-input"):
                complete("input: " + text, "completed")
                continue
            if text == "policy":
                complete("policy: " + policy, "completed")
                continue
            if text == "fail":
                time.sleep(0.4)
            complete("synthetic reply: " + text, "failed" if text == "fail" else "completed")
        elif method == "turn/interrupt":
            time.sleep(0.2)
            if text == "wait-exit": sys.exit(0)
            emit({"id": message["id"], "result": {}})
            complete("synthetic stop", "interrupted")
        elif method is None and message.get("id") == pending:
            emit({"method": "serverRequest/resolved", "params": {"threadId": session, "requestId": pending}})
            complete("synthetic approval handled")
            pending = None
    else:
        if not inspection:
            assert sys.argv[sys.argv.index('--permission-prompt-tool')+1] == 'stdio'
        if message.get("type") == "control_request":
            subtype = message["request"]["subtype"]
            inspection = inspection or message["request_id"] == "panel-inspect-init"
            body={"models":[{"value":"synthetic-claude-model","displayName":"Synthetic Claude","supportedEffortLevels":["low","high"]}],"account":{"email":"private@synthetic.invalid","token":"synthetic-never-transport"}} if inspection else {}
            if inspection:
                assert "--no-session-persistence" in sys.argv
                assert json.loads(sys.argv[sys.argv.index("--mcp-config")+1]) == {"mcpServers":{}}
                assert sys.argv[sys.argv.index("--tools")+1] == ""
                assert json.loads(sys.argv[sys.argv.index("--settings")+1])["disableAllHooks"] is True
            emit({"type": "control_response", "response": {"subtype": "success", "request_id": message["request_id"], "response": body}})
            if subtype == "interrupt" and turn is not None:
                time.sleep(0.2)
                if text == "wait-exit": sys.exit(0)
                complete("synthetic stop", "interrupted")
        elif message.get("type") == "user":
            assert not inspection, "Metadata inspection sent a prompt"
            text = message["message"]["content"]
            turn = message["uuid"]
            emit({"type": "system", "subtype": "init", "session_id": session, "model": "synthetic-claude-model"})
            if text in ("wait", "wait-exit"):
                continue
            if text == "approval":
                pending = "fixture-permission"
                emit({"type": "control_request", "request_id": pending, "request": {"subtype": "can_use_tool", "tool_use_id": "fixture-tool-use", "tool_name": "AskUserQuestion", "input": {"questions": [{"question": "Synthetic question", "header": "Fixture", "options": [{"label": "One", "description": "Synthetic"}, {"label": "Two", "description": "Synthetic"}], "multiSelect": False}]}}})
                emit({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"unrelated-tool","content":"unrelated"}]}})
                continue
            if text == "fail":
                time.sleep(0.4)
            complete("synthetic reply: " + text, "failed" if text == "fail" else "completed")
        elif message.get("type") == "control_response" and message["response"]["request_id"] == pending:
            emit({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"fixture-tool-use","content":"confirmed"}]}})
            complete("synthetic question handled")
            pending = None
