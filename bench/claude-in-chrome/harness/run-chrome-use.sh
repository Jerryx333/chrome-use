#!/bin/sh
# Drive one task with Claude Code + chrome-use and save the stream-json log.
#
#   harness/run-chrome-use.sh <task-id> [session-name]
#
# Reads the prompt from tasks.json, runs `claude -p` with no user settings,
# no MCP and Bash only, then prints a summary (time, turns, tokens, commands).
# CHROME_USE_DIR may point at a directory whose `chrome-use` is a wrapper for
# a remote or isolated install (see README). OUT defaults to ./out.
set -eu
task=$1
session=${2:-bench-$1}
root=$(cd "$(dirname "$0")/.." && pwd)
out=${OUT:-$PWD/out}
mkdir -p "$out" "$out/work"
model=${MODEL:-claude-opus-5-5}
effort=${EFFORT:-medium}
prompt=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["tasks"][sys.argv[2]]["prompt"])' "$root/tasks.json" "$task")
pre='You control a real Chrome browser through the `chrome-use` CLI (already on PATH and connected). Before your first browser action run `chrome-use skills get core` and follow it. Task: '
if [ -n "${CHROME_USE_DIR:-}" ]; then PATH=$CHROME_USE_DIR:$PATH; fi
cd "$out/work"
AGENT_BROWSER_SESSION=$session CVC_SESSION=$session claude -p --setting-sources "" --strict-mcp-config \
  --disable-slash-commands --model "$model" --effort "$effort" --allowedTools "Bash" \
  --output-format stream-json --verbose "$pre$prompt" </dev/null \
  > "$out/cu_$task.jsonl" 2> "$out/cu_$task.err"
python3 "$root/harness/summarize.py" "$out/cu_$task.jsonl" | tee "$out/cu_$task.txt"
