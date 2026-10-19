import XCTest

/// Pairs the app in the simulator with a real `chrome-use mobile serve`
/// (fake panel, demo data) and walks the main screens, saving screenshots.
/// The runner script sets TEST_RUNNER_PAIR_LINK and TEST_RUNNER_SHOTS_DIR.
final class ScreenshotTests: XCTestCase {
    var app: XCUIApplication!
    var shots: URL?

    override func setUp() {
        continueAfterFailure = false
        let env = ProcessInfo.processInfo.environment
        shots = env["SHOTS_DIR"].map { URL(fileURLWithPath: $0) }
        app = XCUIApplication()
        let language = env["SHOTS_LANGUAGE"] ?? "zh-Hans"
        app.launchArguments += ["-AppleLanguages", "(\(language))", "-AppleLocale", language == "en" ? "en_US" : "zh_CN"]
        app.launchEnvironment["CHROMEUSE_UITEST_TRIAL"] = "1"
        if let worker = env["PUSH_WORKER"] { app.launchArguments += ["-pushWorkerURL", worker] }
    }

    func snap(_ name: String) {
        let shot = XCUIScreen.main.screenshot()
        let attachment = XCTAttachment(screenshot: shot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        if let shots {
            try? FileManager.default.createDirectory(at: shots, withIntermediateDirectories: true)
            try? shot.pngRepresentation.write(to: shots.appendingPathComponent("\(name).png"))
        }
    }

    /// The notification permission alert belongs to SpringBoard.
    func allowNotifications() {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        for label in ["允许", "Allow"] {
            let button = springboard.buttons[label]
            if button.waitForExistence(timeout: 3) { button.tap(); return }
        }
    }

    func testWalkthrough() throws {
        let env = ProcessInfo.processInfo.environment
        let link = try XCTUnwrap(env["PAIR_LINK"], "PAIR_LINK is set by the runner")
        let zh = (env["SHOTS_LANGUAGE"] ?? "zh-Hans") != "en"
        func t(_ chinese: String, _ english: String) -> String { zh ? chinese : english }
        let prefix = zh ? "" : "en-"

        app.launch()
        XCTAssertTrue(app.staticTexts[t("还没有配对的电脑", "No paired computers yet")].waitForExistence(timeout: 30))
        snap("\(prefix)01-empty")

        // The scanned / opened link fills the pairing sheet.
        app.open(URL(string: link)!)
        let pair = app.buttons[t("配对", "Pair")]
        XCTAssertTrue(pair.waitForExistence(timeout: 30))
        snap("\(prefix)02-pairing")
        pair.tap()
        allowNotifications()

        let weekly = app.staticTexts["Weekly report"]
        XCTAssertTrue(weekly.waitForExistence(timeout: 40), "conversation list after pairing")
        XCTAssertTrue(app.staticTexts[t("等你处理", "Needs you")].waitForExistence(timeout: 20))
        snap("\(prefix)03-conversations")

        weekly.tap()
        let allow = app.buttons[t("允许一次", "Allow once")]
        XCTAssertTrue(allow.waitForExistence(timeout: 30), "approval card")
        sleep(1)
        snap("\(prefix)04-approval")
        allow.tap()
        // Answered: the service resolves the request and its card goes away.
        let gone = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: allow)
        wait(for: [gone], timeout: 10)
        sleep(1)
        snap("\(prefix)05-approved")

        app.navigationBars.buttons.element(boundBy: 0).tap()
        let compose = app.buttons[t("新建对话", "New conversation")]
        XCTAssertTrue(compose.waitForExistence(timeout: 20))
        compose.tap()
        XCTAssertTrue(app.navigationBars[t("新建对话", "New conversation")].waitForExistence(timeout: 30))
        sleep(2)
        snap("\(prefix)06-new-conversation")
        app.buttons[t("取消", "Cancel")].tap()

        app.buttons[t("定时任务", "Schedules")].tap()
        XCTAssertTrue(app.staticTexts["每日站会摘要"].waitForExistence(timeout: 30))
        snap("\(prefix)07-schedules")
        app.navigationBars.buttons.element(boundBy: 0).tap()
        app.navigationBars.buttons.element(boundBy: 0).tap()

        XCTAssertTrue(app.navigationBars[t("电脑", "Computers")].waitForExistence(timeout: 20))
        snap("\(prefix)08-computers")
        app.buttons[t("设置", "Settings")].tap()
        XCTAssertTrue(app.navigationBars[t("设置", "Settings")].waitForExistence(timeout: 20))
        snap("\(prefix)09-settings")
        app.buttons[t("购买或试用", "Buy or try")].tap()
        XCTAssertTrue(app.staticTexts[t("解锁 chrome-use 遥控", "Unlock chrome-use remote")].waitForExistence(timeout: 20))
        sleep(2)
        snap("\(prefix)10-paywall")
    }
}
