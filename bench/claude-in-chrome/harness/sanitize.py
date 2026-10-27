#!/usr/bin/env python3
"""Strip private data from raw bench logs before committing them.

usage: sanitize.py <in-dir> <out-dir>

- Claude Code stream-json (*.jsonl, *.json with a "type" field): drops hook,
  rate-limit and other system records except a trimmed `init` (model, Claude
  Code version, tools); drops session ids, uuids, thinking signatures, cwd,
  memory paths and plugin lists.
- Every text file: replaces e-mail addresses, home and temp paths, private IPs
  and anything that looks like a token.
- Images are not copied (screenshots can show the signed-in account).
Re-check the output with grep before committing; this is a filter, not proof.
"""
import json
import os
import re
import sys

SUBS = [
    (re.compile(r"[A-Za-z0-9._%+-]+@(?!example\.com)[A-Za-z0-9.-]+\.[A-Za-z]{2,}"), "<email>"),
    (re.compile(r"/(?:private/)?tmp/claude-\d+/[^\s\"'\\]*"), "<tmp>"),
    (re.compile(r"/Users/[^/\s\"'\\]+[^\s\"'\\]*"), "<home>"),
    (re.compile(r"/home/[^/\s\"'\\]+[^\s\"'\\]*"), "<home>"),
    (re.compile(r"/tmp/cvc[^\s\"'\\]*"), "<rig>"),
    (re.compile(r"\b(?:192\.168|10\.\d{1,3})\.\d{1,3}\.\d{1,3}\b"), "<lan-ip>"),
    (re.compile(r"\b(?:sk-ant-|sk-|ghp_|gho_|xox[bp]-)[A-Za-z0-9_-]{8,}"), "<token>"),
    (re.compile(r"Bearer\s+[A-Za-z0-9._-]{8,}"), "Bearer <token>"),
]
DROP_KEYS = {"session_id", "uuid", "signature", "hook_id", "cwd", "original_cwd",
             "memory_paths", "plugins", "messaging_socket_path", "additional_directories",
             "request_id"}
INIT_KEEP = {"type", "subtype", "model", "claude_code_version", "tools", "permissionMode"}


def scrub_text(s):
    for rx, rep in SUBS:
        s = rx.sub(rep, s)
    return s


def scrub(obj):
    if isinstance(obj, dict):
        return {k: scrub(v) for k, v in obj.items() if k not in DROP_KEYS}
    if isinstance(obj, list):
        return [scrub(v) for v in obj]
    if isinstance(obj, str):
        return scrub_text(obj)
    return obj


def keep_record(d):
    t = d.get("type")
    if t == "rate_limit_event":
        return None
    if t == "system":
        if d.get("subtype") != "init":
            return None
        return {k: v for k, v in d.items() if k in INIT_KEEP}
    return d


def main(src, dst):
    os.makedirs(dst, exist_ok=True)
    for name in sorted(os.listdir(src)):
        path = os.path.join(src, name)
        if not os.path.isfile(path) or name.endswith((".png", ".jpg", ".webp")):
            continue
        if os.path.getsize(path) == 0:
            continue
        text = open(path, encoding="utf-8").read()
        if name.endswith(".jsonl"):
            lines = []
            for line in text.splitlines():
                try:
                    d = keep_record(json.loads(line))
                except ValueError:
                    continue
                if d is not None:
                    lines.append(json.dumps(scrub(d), ensure_ascii=False))
            out = "\n".join(lines) + "\n"
        elif name.endswith(".json"):
            d = json.loads(text)
            out = json.dumps(scrub(d), ensure_ascii=False, indent=1) + "\n"
        else:
            out = scrub_text(text)
        open(os.path.join(dst, name), "w", encoding="utf-8").write(out)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
