# Inspect & Ask acceptance

Real-browser acceptance for Inspect & Ask, run on the build host only (never on
the maintainer's machine). It drives Chrome for Testing through CDP with the
unpacked extension, the branch's native host and a fake `ocs` that records each
`dm`. Every expectation is an assert; any failure exits non-zero and
`results.json` keeps the details.

Isolation: the native host manifest lives in the run's own `--user-data-dir`,
and `nm-host.sh` gives the host its own `HOME`, so the build host's real
chrome-use relay and `~/.chrome-use` are untouched. The acceptance copy of the
extension adds `host_permissions` in place of the activeTab grant a real
keyboard shortcut gives; the shortcut path itself is covered by unit tests and
a headed check.

Setup on the build host:

```sh
rsync -a scripts/inspect-acceptance/ <host>:inspect-accept/
rsync -a --exclude '*.test.js' extensions/ab-connect/ <host>:inspect-accept/ext/
scp <branch chrome-use binary> <host>:inspect-accept/bin/chrome-use
ssh <host> 'cd inspect-accept/react-app && npm install && npm run build && npm run build:test'
ssh <host> 'cd inspect-accept/vue-app && npm install'
ssh <host> 'cd inspect-accept && ./run.sh'          # ONLY=name1,name2 to run a subset
```

Scenarios: first enable, bind and send (context file permissions, no password
values, PNG screenshot, no resend of a locked batch); multi-select and pause;
picking never clicks buttons or follows links, and the page works after exit;
copied frame plus init flood; overlay blocks send; background tab capture is
refused and can be retaken; unknown delivery is never resent; stored and wake
shown apart; Vue source and Codex session; test-environment marks and revision;
production build without marks and the consent-gated setup request; Copy; reload
revokes the frame.
