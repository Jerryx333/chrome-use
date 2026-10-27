# Teach by demonstration (`chrome-use teach`)

When the user would rather **show** a flow than describe it, record it once
and save it as a script that replays with no model in the loop.

```bash
chrome-use open https://shop.example/cart        # the page the flow starts on
chrome-use teach start --name checkout           # then the user does the flow by hand in that tab
chrome-use teach status                          # how many actions so far
chrome-use teach stop                            # writes ~/.chrome-use/teach/checkout.json, prints the steps
chrome-use teach replay ~/.chrome-use/teach/checkout.json --arg-env password=SHOP_PW
```

- Ask the user to do the flow **in the session's tab** (the one you opened).
  Clicks, typing, selects, checkboxes, Enter/Escape, uploads and navigations
  are recorded; scrolls only when they made the page load more.
- `teach stop` prints the steps. **Show them to the user** before relying on
  the script, and fix anything odd in the JSON (it is a normal `script` op-list).
- `teach replay <file>` is `chrome-use script <file>`; it exits 1 on the first
  step that fails, naming it.
- `teach cancel` throws the recording away. `teach stop --print` prints the
  script instead of writing it.

## What the script looks like

Each element step leads with a semantic locator and lists alternatives in
`or`, tried only when the one before found nothing to act on (never after a
step that acted):

```json
{"do": "getbylabel", "label": "Email", "subaction": "fill", "value": "ada@example.com",
 "or": [{"do": "getbyrole", "role": "textbox", "name": "Email", "subaction": "fill", "value": "ada@example.com"},
        {"do": "fill", "selector": "#email", "value": "ada@example.com"}]}
```

A click that led to a new page is followed by `waitforurl` (scheme, host and
path) and `waitforloadstate`. The script ends by asserting the final URL.

## Secrets and files

Values typed into password, card and one-time-code fields **never leave the
page**: the recorder reports only that a secret was typed. They become
parameters, guarded by an assert at the top of the script:

```bash
chrome-use teach replay flow.json --arg-env password=SHOP_PW   # read from the environment
chrome-use teach replay flow.json --arg file=./invoice.pdf      # an upload
```

Prefer `--arg-env` for secrets so they stay out of shell history and `ps`.
A GET form that puts the secret in the next URL has it blanked in the recording.

## As a site adapter

```bash
chrome-use teach stop --adapter        # ~/.chrome-use/my-sites/<site>/<flow>.js
chrome-use site add ~/.chrome-use/my-sites && chrome-use site update
chrome-use site <site>/<flow> --until-done --password @-
```

The skeleton uses `args.ui` (trusted input) and runs one page per call, so
run it with `--until-done`. Typed values become optional args (`text1`, …)
with the recorded value as default. Review it like any adapter
(`core/site-adapters`).

`chrome-use teach compile <name>.recording.json [--adapter]` compiles a saved
raw recording again.

## Stealth

The recorder runs in an isolated world made for this recording; it reports
through a CDP binding added only to that world. The page's own world gets no
global, no DOM node and no listener, and `teach stop` removes the recorder
and the binding. Because Chrome only re-adds a binding to a new document when
`Runtime.enable` is on (which pages can detect), the recorder is reinstalled
after each navigation: an action in the first instant of a new page can be
missed, which is why the user should review the steps. Older extensions in
stealth mode refuse the binding; the recorder then falls back to polling and
says so.

The side panel's **Record** button does the same for the panel's tab and
shows the steps for review before saving.
