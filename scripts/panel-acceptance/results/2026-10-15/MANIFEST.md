# Verifiable panel turns: live acceptance (plan 14.2)

Build host 192.168.0.190, 2026-10-15. Driver, fixtures and method: `scripts/panel-acceptance/README.md`.

| item | value |
|---|---|
| host binary (`36605e69`, debug profile) sha256 | `sha256:68dce731ed9b29406e01ca7da6a56b2914536e8cb2c265058506653ff57a92ec` |
| host binary (`36605e69-e2e`, debug profile) sha256 | `sha256:c2c5476b8ea0a03107e14057ebaa6a0d19a70a1b768a14fef05742222c1e80ec` |
| extension tree hash (extensions/ab-connect 0.5.57, tests excluded) | `97be9e1a1a7c93d8b5a428a7cddff1dcd733e22c0f1ab22b3077159a56f85f65` |
| Chrome for Testing | Chrome/155.0.8059.39 (headless=new, private --user-data-dir) |
| v1.5.214 release binary sha256 (checks 15, 16) | `sha256:d77538717467a0fd84b71b4465376d52998b67fa6e641dc0ad1db53b76a7bb41` |
| v1.5.214 extension tree hash (checks 15) | `815c3c3b7e088d7580f574fe1ceca9124478f7866e13dd945e43519efe464ffc` |
| result files | `results.json` (2026-10-15T04:02:13.870Z), `results-5-15.json` (2026-10-15T04:20:21.282Z), `results-check5-e2e.json` (2026-10-15T04:21:51.728Z) |

| check | name | status | seconds | from | note / error |
|---|---|---|---|---|---|
| 1 | Tool failure then recovery | pass | 6.9 | results.json |  |
| 2 | Empty answer | pass | 5.1 | results.json |  |
| 3 | Partial completion | pass | 5.3 | results.json |  |
| 4 | A, B, C pages | pass | 6.5 | results.json |  |
| 5 | Stop timeout with process alive | pass | 21.1 | results-5-15.json | release build: kill path, and dead leader with a live group through review, terminate, fork and a service restart. The leader-alive path needs the e2e build: results-check5-e2e.json. The Windows half needs the Windows runner and was not run. |
| 5 (e2e build) | Stop timeout with process alive | fail | 11.2 | results-check5-e2e.json | Error: leaderAlive: review proves the group alive |
| 6 | Approval/stop interleave | pass | 14 | results.json |  |
| 7 | Same-name downloads | pass | 1.4 | results.json |  |
| 8 | Interrupted download | pass | 14.4 | results.json |  |
| 9 | Deleted artifact file | pass | 0 | results.json |  |
| 10 | Cross-conversation artifact read | pass | 0 | results.json |  |
| 11 | Synthetic secrets in the journal and state | pass | 2.6 | results.json |  |
| 12 | Stalled progress | pass | 200.6 | results.json |  |
| 13 | Refresh resume consistency | pass | 10.6 | results.json |  |
| 14 | Replay producing one turn | pass | 5.2 | results.json | The reply was dropped by the driver proxy on the service socket after the service stored the message; the extension bridge itself was not fault-injected. |
| 15 | Old journal and upgrade retry | pass | 32.3 | results-5-15.json |  |
| 16 | Stealth | pass | 13 | results.json |  |
| 17 | Recording | pass | 17.8 | results.json |  |
| 18 | Turn attribution under load | pass | 13.5 | results.json |  |
| 19 | Assessment counterexamples live | pass | 6.4 | results.json |  |
| 20 | Late Begin across Stop | pass | 52 | results.json |  |
| R | Real backends: scenarios 1, 4 and 12 | pass | 487.3 | results.json |  |

Evidence per check: `checks[].evidence` in the result files. Side-panel screenshots (`panel-*.png`, the real side panel, not a tab) are not committed (`*.png` is git-ignored); they stay next to the results on the build host (`~/panel-accept/repo/scripts/panel-acceptance/results/<date>/`).
