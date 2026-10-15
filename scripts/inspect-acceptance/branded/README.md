# Stealth on branded Chrome

Branded Chrome 155 ignores `--load-extension` (the
`DisableLoadExtensionCommandLineSwitch` escape hatch is gone), and loading
through the CDP pipe is not a valid stealth setup: `--remote-debugging-pipe`
alone makes `navigator.webdriver` true and sannysoft fail, with no extension at
all (`launch-control.mjs`, result in `results/2026-10-15/launch-control.json`).
A CDP-loaded extension also does not survive a relaunch without the pipe.

So the extension is installed the way a developer would: `ui-open.mjs` starts
Chrome on a fresh profile, turns on developer mode and opens "Load unpacked";
the folder picker is then driven with System Events, targeted by pid:

```sh
P=$(cat /tmp/uiinst.pid); S="tell application \"System Events\" to tell (first process whose unix id is $P)"
osascript -e "$S to keystroke \"/\""                 # Go to Folder
osascript -e "$S to set value of text field 1 of sheet 1 of sheet 1 of window 1 to \"$EXT\""
osascript -e "$S to key code 36"
osascript -e "$S to click button \"选择\" of splitter group 1 of sheet 1 of window 1"   # "Select" in English
```

`ui-open.mjs` prints the installed extensions (expect the stable id, UNPACKED,
ENABLED) and closes Chrome gracefully so the install is saved. Then
`PROFILE=<that profile> node rig.mjs <ext> <label>` copies the profile and runs
the relay bench and static-id probes over a port launch.
