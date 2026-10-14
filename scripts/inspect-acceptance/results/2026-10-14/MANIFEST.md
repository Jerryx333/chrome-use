# Inspect & Ask acceptance, 2026-10-14

| Item | Value |
|---|---|
| Source | feat/inspect-and-ask at 5c4cd8c4 (host) with extension tree of 1ef82fcd (no extension change after it) |
| Host binary | macOS arm64, sha256 19e704d76ee9b87bc46e4620dc0b8e655497ceffce6991dc67f39a62e980060a, built on the build host by scripts/remote-cargo.py (receipt d820d0ba75aa4df1885197b36813130d) |
| Browser | Chrome for Testing 155.0.8059.39 on the build host |
| Rust checks | full `test --bin chrome-use` 2076 passed, 0 failed, 3 ignored; `fmt --check` clean; `clippy --all-targets` no new warnings |
| Extension unit tests | 401 passed |

Files:

- `results.json`: 14 headless scenarios, all passed (extension at the final commit; host binary as above). Uses the acceptance copy of the extension (host_permissions in place of the activeTab grant) and the fake ocs, so delivery here is mocked.
- `headed-results.json`: headed Chrome for Testing on the build host's GUI session with the unmodified manifest (activeTab, no host permissions). A real Alt+Shift+C keystroke sent by peekaboo opened the picker; the screenshot was taken through the activeTab grant.
- `real-ocs.json`: the same host binary driven over native messaging on the maintainer's machine, delivering through the real ocs 0.8.3 to a live Claude Code session: state sent, stored true, wake accepted; the same delivery id sent again returned duplicate without calling ocs. The session received the message.

Not covered here: Windows (CI compiles and tests the Windows build), and a single run combining the real shortcut with real ocs on one machine.
