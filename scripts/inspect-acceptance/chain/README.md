# Full chain on one machine

One run of the whole path with nothing stubbed: the released host binary, the
extension zip with its unmodified manifest (activeTab, no host permissions) in
headed Chrome for Testing, a real Alt+Shift+C keystroke (peekaboo), a pick, the
screenshot through the activeTab grant, Send, the real `ocs`, and delivery to a
live agent session that ran `chrome-use inspect link` for `localhost:5899`.

It needs a screen and the maintainer's ocs, so it ran once on the maintainer's
machine with the owner's approval (2026-10-15), in its own Chrome for Testing
profile and an isolated host HOME; the user's Chrome was not touched.

```sh
cp <released chrome-use> chain/chrome-use
unzip extensions/ab-connect.zip -d chain/ext
printf '#!/bin/sh\nHOME=%s exec %s "$@"\n' "$HOME" "$(command -v ocs)" > chain/ocs-real.sh; chmod +x chain/ocs-real.sh
# in the receiving session: HOME=chain/home chrome-use inspect link http://localhost:5899
CHAIN_OCS=$PWD/chain/ocs-real.sh CHROME=<Chrome for Testing binary> node chain/chain.mjs
```
