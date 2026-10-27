# chrome-use vs Claude in Chrome: head-to-head bench

The same tasks, word for word, given to Claude in Chrome (driven through its
own side panel) and to chrome-use (driven by Claude Code), with the same model
and effort. Results feed the "chrome-use vs Claude in Chrome" section of
[compare](https://chrome-use.leeguoo.com/en/compare.html#vs-claude-in-chrome).

## Layout

| Path | What |
|---|---|
| `tasks.json` | Task prompts (t1–t6, parallel, page-side probe) and expected answers |
| `fixtures/a/` | Pages served on `127.0.0.1:8711`: `form.html` (t2), `price.html` + `order.html` (t3), `frame.html` (t4) |
| `fixtures/b/` | `inner.html`, the coupon widget served on `localhost:8712`, so the iframe in `frame.html` is cross-origin |
| `harness/serve.sh` | Serves both fixture origins |
| `harness/run-chrome-use.sh` | Runs one task with `claude -p` + chrome-use, saves stream-json, prints a summary |
| `harness/summarize.py` | Time, turns, tokens, cost and commands from a stream-json log |
| `harness/cic-panel.mjs` | Types a prompt into the Claude in Chrome side panel over CDP and waits for it to finish |
| `harness/remote-chrome-use.sh.example` | Wrapper that runs every chrome-use command on a separate test machine |
| `harness/sanitize.py` | Strips ids, paths, e-mails, IPs and tokens from raw logs before they are committed |
| `data/2026-10-27/` | Raw logs of the first run (sanitized) |

t1 (Hacker News), t5 (botcheck.leeguoo.com) and t6 (bot-detector.rebrowser.net)
use public pages, so their answers change over time.

## Rig

Use throw-away browser profiles on a test machine, never your everyday Chrome:

- **Profile A:** Claude in Chrome from the Web Store, signed in to a Claude
  account with a paid plan. Side panel mode as shipped ("Automatically
  approve" left at the default). Start Chrome with
  `--remote-debugging-port=<port>` so `cic-panel.mjs` can type into the panel.
- **Profile B:** the chrome-use extension plus the release CLI under a
  throw-away `HOME`, with `CHROME_USE_AUTO_UPGRADE=0`. If you install the
  native host with `extension install --user-data-dir`, check that the
  generated launcher sets that `HOME`; otherwise the host writes into the real
  home directory.

## Rerun per release

1. On the test machine: `harness/serve.sh`.
2. Claude in Chrome, for each task:
   `node harness/cic-panel.mjs <port> out/cic_t1.json "<prompt from tasks.json>"`.
   Start a new task in the panel between runs. Record the panel's
   "Used Claude in Chrome (N actions)" count.
3. chrome-use, for each task: `harness/run-chrome-use.sh t1` (and so on).
   Set `CHROME_USE_DIR` to a directory holding a `chrome-use` wrapper if the
   browser runs on another machine (see `remote-chrome-use.sh.example`); that
   adds roughly 0.3–0.6 s of SSH per command.
4. Parallel: start `run-chrome-use.sh t2 pA`, `t4 pB` and `t3 pC` together.
5. botcheck (t5): save the page's JSON report for both tools.
6. Probe: on a fixture page, `fetch()` each extension's web-accessible
   resource and a random extension id from DevTools.
7. `python3 -I harness/sanitize.py out data/<date>`, then grep the result
   for account e-mails, user names and paths before committing. Do not commit
   screenshots of the panel.
8. Update the table below and the compare pages (`docs/compare.html`,
   `docs/en/compare.html`) if a number changed.

## Results, 2026-10-27

chrome-use CLI 1.5.271 with extension 0.5.91; Claude in Chrome 1.0.102.
Branded Chrome 156 on one Mac (M4). Opus 5.5, medium effort, on both sides:
Claude in Chrome's panel showed "Opus 5.5 · Medium"; chrome-use ran under
Claude Code 2.1.311 (`--model claude-opus-5-5 --effort medium`, Bash only, no
user settings or MCP). One run per task, so use the numbers for direction only.
Claude in Chrome times are ±3 s (2 s polling).

| Task | Claude in Chrome: time / actions / result | chrome-use: time / browser calls / result |
|---|---|---|
| t1 HN top 30 → JSON | 38 s / 5 / 30 items, rendered with typographic quotes | 28 s / 2 / 30/30, valid JSON |
| t2 9-field form | 22 s / 4 / correct | 21 s / n/a / correct, identical submitted JSON |
| t3 two tabs → PO | 18 s / 4 / correct | 29 s / 5 (+1 failed `tab select 0`) / correct |
| t4 cross-origin iframe coupon | 30 s / 6 / applied, but read from a screenshot and returned `SPRING−25` (U+2212) | 24 s / 3 / exact `APPLIED:SPRING-25` |
| t5 botcheck, 4 on-page actions | 48 s / 7 / 14 pass, 5 fail | 30 s / 4 / 18 pass, 1 fail |
| t6 rebrowser bot-detector | 44 s / 2 / 16 green, 0 red, 4 neutral | 13 s / 2 / 16 green, 0 red, 4 neutral |
| parallel t2 + t4 + t3 | not run | 3/3 correct, ~31 s wall clock |
| page-side probe | web-accessible script fetch → HTTP 200 | same failure as a random extension id |

Notes:

- botcheck failures. Claude in Chrome: `injected-dom`, `window-geometry`,
  `ime-composition`, `key-dwell` (~0 ms), `select-change-trust`. chrome-use:
  `window-geometry` only, because both drive a background tab. botcheck is
  written by the chrome-use maintainer; rebrowser (t6) is independent.
- chrome-use input tokens were 116k–237k per task, mostly cache reads; about
  6.5k of that was `skills get core` (26 KB in 1.5.271). Claude in Chrome
  exposes no per-task token counts.
- t3 on Claude in Chrome ran in the same conversation as t2, so it had some
  context carried over.
- Not tested: `claude --chrome`, Claude in Chrome concurrency.
- File names in `data/2026-10-27/`: `cic_t*.json` are panel transcripts
  (`secs`, approval prompts seen, panel text); `cic_botcheck.json` is
  botcheck's report for Claude in Chrome; `cu_t*.jsonl` and `cu_p*.jsonl` are
  Claude Code stream-json logs; `pA/pB/pC.txt` are the parallel-run summaries.
  `cu_t2.json` is a plain `--output-format json` result, so its command list
  was not captured. The rebrowser screenshot for Claude in Chrome is not
  published.
