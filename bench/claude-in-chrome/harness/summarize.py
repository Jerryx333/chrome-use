#!/usr/bin/env python3
"""Summarize a Claude Code stream-json log: time, turns, tokens, cost and the
Bash commands the agent ran, then the start of its final answer.

usage: summarize.py cu_<task>.jsonl
"""
import json
import sys

cmds, res = [], None
for line in open(sys.argv[1], encoding="utf-8"):
    try:
        d = json.loads(line)
    except ValueError:
        continue
    if d.get("type") == "assistant":
        for c in d["message"].get("content", []):
            if c.get("type") == "tool_use":
                cmds.append(c["input"].get("command", "")[:160])
    if d.get("type") == "result":
        res = d
if res is None:
    sys.exit("no result record: the run did not finish")
u = res["usage"]
print("secs", res["duration_ms"] / 1000, "turns", res["num_turns"], "tool_calls", len(cmds),
      "in", u["input_tokens"], "cache_create", u["cache_creation_input_tokens"],
      "cache_read", u["cache_read_input_tokens"], "out", u["output_tokens"],
      "cost", round(res["total_cost_usd"], 3))
for c in cmds:
    print("  $", c.replace("\n", " ; "))
print(res["result"][:900])
