import ChromeUseKit
import StoreKitTest
import XCTest
@testable import ChromeUse

/// Runs in the simulator with the app as host, so the String Catalog is the
/// real compiled one.
final class CloseMessageTests: XCTestCase {
    let reasons: [CloseReason] = [
        CloseReason(closeCode: 4400), CloseReason(closeCode: 4401), CloseReason(closeCode: 4403),
        CloseReason(closeCode: 4408), CloseReason(closeCode: 4409), CloseReason(closeCode: 4429),
    ]

    func testEveryCloseCodeHasItsOwnChineseMessage() {
        let messages = reasons.map(\.message)
        XCTAssertEqual(Set(messages).count, messages.count, "each close code says something different")
        for message in messages {
            XCTAssertTrue(message.unicodeScalars.contains { (0x4E00...0x9FFF).contains($0.value) }, "\(message) is Chinese")
        }
        XCTAssertTrue(CloseReason(closeCode: 4403).message.contains("撤销"))
        XCTAssertTrue(CloseReason(closeCode: 4403).message.contains("重新配对"))
        XCTAssertTrue(CloseReason(closeCode: 4409).message.contains("--profile"))
    }

    func testEnglishLocalizationExists() throws {
        let path = try XCTUnwrap(Bundle.main.path(forResource: "en", ofType: "lproj"), "en.lproj is in the app")
        let english = try XCTUnwrap(Bundle(path: path))
        for key in ["这部手机已在电脑上被撤销，请重新配对。", "等你处理", "允许一次", "配对电脑"] {
            let translated = english.localizedString(forKey: key, value: "__missing__", table: nil)
            XCTAssertNotEqual(translated, "__missing__", key)
            XCTAssertFalse(translated.unicodeScalars.contains { (0x4E00...0x9FFF).contains($0.value) }, "\(key) → \(translated)")
        }
    }

    func testDecisionLabelsCoverEveryKind() {
        let kinds: [DecisionOption.Kind] = [.allowOnce, .allowForSession, .allowTurn, .deny, .decline, .cancel]
        for kind in kinds {
            XCTAssertFalse(DecisionOption(kind: kind, decision: "x").label.isEmpty)
        }
    }
}

@MainActor
final class StoreEntitlementTests: XCTestCase {
    func testPaywallGateFollowsEntitlement() {
        let start = Date(timeIntervalSince1970: 1_800_000_000)
        XCTAssertTrue(Entitlement.evaluate(unlocked: false, trialStart: start, clock: FixedClock(start.addingTimeInterval(86_400))).canWrite)
        XCTAssertFalse(Entitlement.evaluate(unlocked: false, trialStart: start, clock: FixedClock(start.addingTimeInterval(15 * 86_400))).canWrite)
        XCTAssertTrue(Entitlement.evaluate(unlocked: true, trialStart: start, clock: FixedClock(start.addingTimeInterval(15 * 86_400))).canWrite)
    }
}


/// Purchases against App/Products.storekit in a StoreKit test session: the
/// trial's purchase date starts the 14 days, the unlock wins over it.
@MainActor
final class StorePurchaseTests: XCTestCase {
    func testTrialThenUnlock() async throws {
        let session = try SKTestSession(configurationFileNamed: "Products")
        session.resetToDefaultState()
        session.disableDialogs = true
        session.clearTransactions()
        let store = Store()
        await store.load()
        let trial = try XCTUnwrap(store.trialProduct, "trial product loads")
        let unlock = try XCTUnwrap(store.unlockProduct, "unlock product loads")
        XCTAssertEqual(unlock.price, Decimal(string: "1.99"))
        XCTAssertEqual(trial.price, 0)
        XCTAssertEqual(store.entitlement, .trialNotStarted)
        XCTAssertFalse(store.entitlement.canWrite)

        await store.buy(trial)
        let started = try XCTUnwrap(store.trialStart)
        XCTAssertLessThan(abs(started.timeIntervalSinceNow), 120)
        XCTAssertEqual(store.entitlement, .trial(endsAt: started.addingTimeInterval(Entitlement.trialLength)))
        XCTAssertTrue(store.entitlement.canWrite)

        await store.buy(unlock)
        XCTAssertEqual(store.entitlement, .unlocked)
        session.clearTransactions()
    }
}

/// The app's entitlements let it use the shared keychain group the
/// notification extension reads push keys from.
final class SharedKeychainTests: XCTestCase {
    func testCredentialRoundTripInSharedGroup() throws {
        let group = try XCTUnwrap(Bundle.main.object(forInfoDictionaryKey: "ChromeUseKeychainGroup") as? String)
        XCTAssertFalse(group.isEmpty)
        let store = KeychainCredentialStore(service: "chrome-use.gateway.test", accessGroup: group)
        let credential = GatewayCredential(url: URL(string: "ws://127.0.0.1:17345/")!, deviceId: UUID().uuidString.lowercased(), secret: Data(repeating: 5, count: 64), gatewayId: "g", name: "Mac", pairedAt: Date())
        try store.save(credential)
        XCTAssertEqual(try store.all().first { $0.deviceId == credential.deviceId }, credential)
        var renamed = credential
        renamed.name = "Mac mini"
        try store.save(renamed)
        XCTAssertEqual(try store.all().filter { $0.deviceId == credential.deviceId }, [renamed])
        try store.delete(deviceId: credential.deviceId)
        XCTAssertFalse(try store.all().contains { $0.deviceId == credential.deviceId })
    }
}
