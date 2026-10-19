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

@MainActor
final class RepairFlowTests: XCTestCase {
    let gateway = Base64URL.encode(Data(repeating: 9, count: 16))

    func credential() -> GatewayCredential {
        GatewayCredential(url: URL(string: "ws://127.0.0.1:9/")!, deviceId: UUID().uuidString.lowercased(), secret: Data(repeating: 1, count: 64), gatewayId: gateway, name: "Studio", pairedAt: Date())
    }

    func link() -> PairLink {
        PairLink(url: URL(string: "ws://127.0.0.1:9/")!, deviceId: UUID().uuidString.lowercased(), pairSecret: Data(repeating: 2, count: 32), gatewayId: gateway)
    }

    /// A revoked computer (4403) offers re-pairing; pairing it again replaces
    /// the old credential and keeps the user's name for it.
    func testRevokedComputerIsReplacedByRepairing() async throws {
        let old = credential()
        let keychain = InMemoryCredentialStore([old])
        let model = AppModel(credentials: keychain, cache: InMemoryJournalCache()) { link, store in
            let secret = link.pairSecret + Data(repeating: 0xAB, count: 32)
            try store(secret)
            return (nil, secret)
        }
        model.asksForNotifications = false
        XCTAssertEqual(model.clients.map(\.id), [old.deviceId])
        XCTAssertTrue(CloseReason(closeCode: 4403).offersRepair)

        model.startRepair(old.deviceId)
        XCTAssertTrue(model.showPairing)
        XCTAssertEqual(model.repairing, old.deviceId)
        let fresh = link()
        let client = try await model.pair(fresh, name: "Studio")
        XCTAssertEqual(model.clients.map(\.id), [fresh.deviceId])
        let stored = try keychain.all()
        XCTAssertEqual(stored.map(\.deviceId), [fresh.deviceId])
        XCTAssertEqual(stored.first?.secret, fresh.pairSecret + Data(repeating: 0xAB, count: 32))
        XCTAssertEqual(stored.first?.name, "Studio")
        XCTAssertNil(model.repairing)
        model.remove(client.id, tellGateway: false)
    }

    /// A refused re-pair (spent or expired QR: 4401) keeps the old entry and
    /// leaves no half-stored credential behind.
    func testRefusedRepairingKeepsTheOldComputer() async throws {
        let old = credential()
        let keychain = InMemoryCredentialStore([old])
        let model = AppModel(credentials: keychain, cache: InMemoryJournalCache()) { link, store in
            try store(link.pairSecret + Data(repeating: 0xCD, count: 32))
            throw GatewayError.closed(.unauthorized)
        }
        model.asksForNotifications = false
        model.startRepair(old.deviceId)
        do {
            _ = try await model.pair(link(), name: "Studio")
            XCTFail("pairing was refused")
        } catch {
            XCTAssertEqual(error as? GatewayError, .closed(.unauthorized))
        }
        XCTAssertEqual(try keychain.all().map(\.deviceId), [old.deviceId])
        XCTAssertEqual(model.clients.map(\.id), [old.deviceId])
        XCTAssertEqual(model.repairing, old.deviceId, "the sheet can retry with a new QR")
        model.remove(old.deviceId, tellGateway: false)
    }
}

@MainActor
final class NotificationRouteTests: XCTestCase {
    static let payload = "D9OaRmBhYmNkZWZnaGlqa5LWr9sMfSfsQKyuDrzA3lU/VPPJ6gqLhPXaVYvov7CkcK80I1Lyu2LV3h1400pohwLJxAkRdNJDpBOB6YFXBj+KqnEkTNyedZ9NlyimW4JZCLDWHVkisCr6vRg+bP0LTJ9IPNZ/vuEAUEvASnRUoP0fA2z1ChvjIk0tBepBrmiuVxuePSZeJEuygSCkNnBbEVQAqk2LryQAdNZH2Fn72U5ucwevM2p9auOluknJ1H1AWrc="

    /// The extension decrypts the spec's push vector and merges its route;
    /// tapping the notification opens that conversation (its approval card
    /// sits at the bottom of the conversation view).
    func testTapOpensTheConversation() throws {
        let device = UUID().uuidString.lowercased()
        let credential = GatewayCredential(url: URL(string: "ws://127.0.0.1:9/")!, deviceId: device, secret: Data(0x00...0x1f) + Data(0x80...0x9f), gatewayId: "g", name: "Mac", pairedAt: Date())
        let model = AppModel(credentials: InMemoryCredentialStore([credential]), cache: InMemoryJournalCache())
        let notice = try XCTUnwrap(PushNotice.open(payload: Self.payload, credentials: [credential]))
        XCTAssertEqual(notice.code, .needsInput)
        let conversation = "7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e"

        model.path = [.schedules(device)]
        model.openNotification(userInfo: notice.userInfo)
        XCTAssertEqual(model.path, [.computer(device), .conversation(device, conversation)])

        // A test push has no conversation: open the computer.
        model.openNotification(userInfo: ["deviceId": device, "conversationId": "", "kind": "test"])
        XCTAssertEqual(model.path, [.computer(device)])

        // Unknown computer (removed since) or the generic fallback: stay put.
        model.openNotification(userInfo: ["deviceId": UUID().uuidString, "conversationId": conversation])
        model.openNotification(userInfo: [:])
        XCTAssertEqual(model.path, [.computer(device)])
        model.remove(device, tellGateway: false)
    }
}

@MainActor
final class ComputerNameAndTokenTests: XCTestCase {
    func testRenameSticksAndRejectionRegistersAgain() throws {
        let credential = GatewayCredential(url: URL(string: "ws://127.0.0.1:9/")!, deviceId: UUID().uuidString.lowercased(), secret: Data(repeating: 1, count: 64), gatewayId: "g", name: "127.0.0.1", pairedAt: Date())
        let keychain = InMemoryCredentialStore([credential])
        let model = AppModel(credentials: keychain, cache: InMemoryJournalCache())
        let client = try XCTUnwrap(model.client(credential.deviceId))

        // The gateway's name replaces the URL host and is saved.
        client.applyHello(["computerName": "Studio Mac", "gatewayVersion": "1.5.250"])
        XCTAssertEqual(try keychain.all().first?.name, "Studio Mac")
        XCTAssertEqual(try keychain.all().first?.gatewayVersion, "1.5.250")
        // A name the user typed wins from then on.
        model.rename(credential.deviceId, to: "工作室")
        client.applyHello(["computerName": "Studio Mac", "gatewayVersion": "1.5.251"])
        XCTAssertEqual(client.credential.name, "工作室")
        XCTAssertEqual(try keychain.all().first?.name, "工作室")
        XCTAssertEqual(try keychain.all().first?.nameIsCustom, true)

        // A refused relay token makes the app register with the Worker again.
        client.handle(["event": "pushTokenRejected", "reason": "invalid"])
        XCTAssertNil(model.push.registration)
        XCTAssertTrue(model.push.lastError?.contains("invalid") == true)
        model.remove(credential.deviceId, tellGateway: false)
    }
}
