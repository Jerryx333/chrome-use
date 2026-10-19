# chrome-use remote for iOS

SwiftUI app (iOS 17+) that drives the desktop agent panel through `chrome-use mobile serve`.
Protocol and crypto follow `docs/superpowers/specs/2026-10-19-mobile-remote-design.md`.

- `ChromeUseKit/`: Swift package with no UIKit dependency: key schedule, handshake, frame channel,
  push decryption, pairing links, the WebSocket connection and reconnecting client, the journal
  timeline, approval forms, entitlement logic and image preparation. Runs under `swift test` on macOS.
- `App/`: the SwiftUI app (pairing, conversations, approvals, schedules, push, StoreKit 2, Face ID lock).
- `NotificationService/`: decrypts the gateway's push payload and rewrites the notification.
- `Config/Identity.xcconfig`: bundle id, team, app group, keychain group, push Worker URL and product
  ids. Change them there only.

## Build and test

Generate the project with XcodeGen (the `.xcodeproj` is not committed):

```sh
xcodegen generate
```

Package tests, including the end-to-end test against a real gateway (it starts
`Tests/ChromeUseKitTests/Fixtures/fake_panel.py` as the panel service and push Worker, in a temporary
HOME and `CHROME_USE_RELAY_DIR`):

```sh
cd ChromeUseKit
CHROME_USE_E2E_BIN=/path/to/chrome-use swift test
```

App unit tests (close-code messages, localization, StoreKit purchases against `App/Products.storekit`,
shared keychain) on a simulator:

```sh
xcodebuild -project ChromeUse.xcodeproj -scheme ChromeUse \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' test -only-testing:ChromeUseTests
```

`scripts/simulator-walkthrough.sh [zh-Hans|en]` runs the UI walkthrough against a demo gateway and
writes the screenshots in `docs/screenshots/`.
