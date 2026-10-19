import ChromeUseKit
import StoreKitTest
import XCTest
@testable import ChromeUse

/// Runs in the simulator with the app as host, so these are the compiled
/// catalogs. The test action runs in English (the source language).
final class CloseMessageTests: XCTestCase {
    static let locales = ["en", "zh-Hans", "zh-Hant", "ja"]
    let reasons: [CloseReason] = [
        CloseReason(closeCode: 4400), CloseReason(closeCode: 4401), CloseReason(closeCode: 4403),
        CloseReason(closeCode: 4408), CloseReason(closeCode: 4409), CloseReason(closeCode: 4429),
    ]

    func testEveryCloseCodeHasItsOwnMessage() {
        let messages = reasons.map(\.message)
        XCTAssertEqual(Set(messages).count, messages.count, "each close code says something different")
        XCTAssertTrue(CloseReason(closeCode: 4403).message.contains("revoked"))
        XCTAssertTrue(CloseReason(closeCode: 4409).message.contains("--profile"))
    }

    /// Every shipped language is in the bundle and translates the messages
    /// (the source catalogs are checked key by key in ChromeUseKit's
    /// AppCatalogTests).
    func testEveryLanguageIsCompiledIn() throws {
        XCTAssertEqual(Set(Bundle.main.localizations).intersection(Self.locales), Set(Self.locales))
        let keys = ["This phone was revoked on the computer. Pair again.", "Needs you", "Allow once", "Pair a computer",
                    "Start 14-day free trial", "Untitled conversation"]
        for locale in Self.locales where locale != "en" {
            let path = try XCTUnwrap(Bundle.main.path(forResource: locale, ofType: "lproj"), "\(locale).lproj")
            let bundle = try XCTUnwrap(Bundle(path: path))
            for key in keys {
                let value = bundle.localizedString(forKey: key, value: "__missing__", table: nil)
                XCTAssertNotEqual(value, "__missing__", "\(locale): \(key)")
                XCTAssertNotEqual(value, key, "\(locale) translates \(key)")
            }
            let usage = bundle.localizedString(forKey: "NSCameraUsageDescription", value: "__missing__", table: "InfoPlist")
            XCTAssertNotEqual(usage, "__missing__", "\(locale): camera usage description")
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
        #if !targetEnvironment(simulator)
        // On a device SKTestSession waits for the App Store sign-in sheet.
        throw XCTSkip("StoreKit test sessions run in the simulator only")
        #endif
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

/// Full credentials live in the app's own keychain group; the group shared
/// with the notification extension holds push keys only.
final class KeychainSplitTests: XCTestCase {
    var appGroup: String { Bundle.main.object(forInfoDictionaryKey: "ChromeUseKeychainAppGroup") as? String ?? "" }
    var sharedGroup: String { Bundle.main.object(forInfoDictionaryKey: "ChromeUseKeychainGroup") as? String ?? "" }

    func credential() -> GatewayCredential {
        GatewayCredential(url: URL(string: "ws://127.0.0.1:17345/")!, deviceId: UUID().uuidString.lowercased(), secret: Data(repeating: 5, count: 64), gatewayId: "test-\(UUID().uuidString.prefix(6))", name: "Mac", pairedAt: Date())
    }

    func testCredentialsStayInTheAppGroupAndPushKeysAreShared() throws {
        XCTAssertFalse(appGroup.isEmpty)
        XCTAssertNotEqual(appGroup, sharedGroup)
        let store = KeychainCredentialStore(service: "chrome-use.computer.test", accessGroup: appGroup)
        let keys = KeychainPushKeyStore(service: "chrome-use.pushkey.test", accessGroup: sharedGroup)
        let saved = credential()
        try store.save(saved)
        try keys.save(PushKeyEntry(credential: saved))
        defer { try? store.delete(deviceId: saved.deviceId); try? keys.delete(deviceId: saved.deviceId) }

        let item = try XCTUnwrap(store.items.all().first { $0.account == saved.storageKey })
        XCTAssertEqual(item.group, appGroup, "the full secret is in the app-only group")
        // What the extension can query (the shared group) has no credential.
        XCTAssertTrue(try KeychainItems(service: "chrome-use.computer.test", accessGroup: sharedGroup).all().isEmpty)
        let entry = try XCTUnwrap(keys.all().first { $0.deviceId == saved.deviceId })
        XCTAssertEqual(entry.pushKey, saved.keys.push)
        XCTAssertEqual(entry.kid, PushCrypto.kid(pushKey: saved.keys.push))
        let pushItem = try XCTUnwrap(keys.items.all().first { $0.account == saved.storageKey })
        XCTAssertEqual(pushItem.group, sharedGroup)
        XCTAssertFalse(String(decoding: pushItem.data, as: UTF8.self).contains(saved.secret.base64EncodedString()))

        var renamed = saved
        renamed.name = "Mac mini"
        try store.save(renamed)
        XCTAssertEqual(try store.all().filter { $0.deviceId == saved.deviceId }, [renamed])
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

@MainActor
final class PairingConflictTests: XCTestCase {
    func testLinkReusingADeviceIdForAnotherComputerNeedsConfirmation() async throws {
        let device = UUID().uuidString.lowercased()
        let saved = GatewayCredential(url: URL(string: "ws://100.64.1.2:17345/")!, deviceId: device, secret: Data(repeating: 1, count: 64), gatewayId: "home", name: "Home Mac", pairedAt: Date())
        let keychain = InMemoryCredentialStore([saved])
        let pushKeys = InMemoryPushKeyStore()
        try pushKeys.save(PushKeyEntry(credential: saved))
        let model = AppModel(credentials: keychain, pushKeys: pushKeys, cache: InMemoryJournalCache()) { link, store in
            let secret = link.pairSecret + Data(repeating: 0xEE, count: 32)
            try store(secret)
            return (nil, secret)
        }
        model.asksForNotifications = false

        // Same device id and computer: an ordinary re-scan, no conflict.
        XCTAssertNil(model.conflict(for: PairLink(url: saved.url, deviceId: device, pairSecret: Data(count: 32), gatewayId: "home")))
        // Same device id, another address or gateway: a conflict.
        let other = PairLink(url: URL(string: "ws://192.168.1.9:17345/")!, deviceId: device, pairSecret: Data(repeating: 4, count: 32), gatewayId: "office")
        XCTAssertEqual(model.conflict(for: other), saved)
        XCTAssertNotNil(model.conflict(for: PairLink(url: saved.url, deviceId: device, pairSecret: Data(count: 32), gatewayId: "office")))

        do {
            _ = try await model.pair(other, name: "Office")
            XCTFail("a conflicting link must not pair without confirmation")
        } catch is AppModel.PairingConflict {}
        XCTAssertEqual(try keychain.all(), [saved])

        // Confirmed: the new computer replaces the old one, keys included.
        _ = try await model.pair(other, name: "Office", replacing: true)
        let stored = try keychain.all()
        XCTAssertEqual(stored.map(\.gatewayId), ["office"])
        XCTAssertEqual(try pushKeys.all().map(\.gatewayId), ["office"])
        XCTAssertEqual(model.clients.map(\.credential.gatewayId), ["office"])
        model.remove(device, tellGateway: false)
    }
}

final class MutableClock: Clock, @unchecked Sendable {
    var now: Date
    init(_ now: Date) { self.now = now }
}

@MainActor
final class AppLockTests: XCTestCase {
    func defaults() -> UserDefaults {
        let name = "lock-test-\(UUID().uuidString)"
        return UserDefaults(suiteName: name)!
    }

    func testRequiredLockAtLaunchAndAfterFiveMinutes() async {
        let clock = MutableClock(Date(timeIntervalSince1970: 1_800_000_000))
        var prompts = 0
        let lock = AppLock(defaults: defaults(), clock: clock, passcodeSet: { true }) { _ in prompts += 1; return true }
        XCTAssertFalse(lock.locked)
        lock.setRequired(true, atLaunch: true)
        XCTAssertTrue(lock.locked, "required: locked on launch")
        await lock.unlock()
        XCTAssertFalse(lock.locked)
        XCTAssertEqual(prompts, 1)

        lock.background()
        clock.now += 4 * 60
        lock.foreground()
        XCTAssertFalse(lock.locked, "under five minutes away: still open")
        lock.background()
        clock.now += 5 * 60 + 1
        lock.foreground()
        XCTAssertTrue(lock.locked, "over five minutes away: locked")

        // It cannot be switched off while a computer requires it.
        await lock.unlock()
        await lock.setEnabled(false)
        XCTAssertTrue(lock.active)
    }

    func testOptionalLockLocksOnEveryReturn() async {
        let clock = MutableClock(Date(timeIntervalSince1970: 1_800_000_000))
        let lock = AppLock(defaults: defaults(), clock: clock, passcodeSet: { true }) { _ in true }
        await lock.setEnabled(true)
        lock.background()
        lock.foreground()
        XCTAssertTrue(lock.locked)
    }

    /// `hello.fullAccessAllowed` from any computer turns the requirement on.
    func testHelloMakesTheLockRequired() {
        let credential = GatewayCredential(url: URL(string: "ws://127.0.0.1:9/")!, deviceId: UUID().uuidString.lowercased(), secret: Data(repeating: 1, count: 64), gatewayId: "g", name: "Mac", pairedAt: Date())
        let keychain = InMemoryCredentialStore([credential])
        let lock = AppLock(defaults: defaults(), clock: SystemClock(), passcodeSet: { true }) { _ in true }
        let model = AppModel(credentials: keychain, cache: InMemoryJournalCache(), lock: lock)
        XCTAssertFalse(lock.required)
        model.client(credential.deviceId)?.applyHello(["computerName": "Mac", "gatewayVersion": "1", "pushMutes": [], "fullAccessAllowed": true])
        XCTAssertTrue(lock.required)
        XCTAssertEqual(try keychain.all().first?.fullAccessAllowed, true, "remembered for the next launch")
        let relaunched = AppModel(credentials: keychain, cache: InMemoryJournalCache(), lock: AppLock(defaults: defaults(), clock: SystemClock(), passcodeSet: { true }) { _ in true })
        XCTAssertTrue(relaunched.lock.locked)
        model.remove(credential.deviceId, tellGateway: false)
        relaunched.remove(credential.deviceId, tellGateway: false)
    }

    func testAccessTooHighNamesTheCommand() {
        let message = GatewayError.remote(code: "accessTooHigh", message: "x").message(deviceId: "0f8fad5b-d9cb-469f-a165-70867728950e")
        XCTAssertTrue(message.contains("chrome-use mobile access 0f8fad5b-d9cb-469f-a165-70867728950e full"), message)
    }
}

@MainActor
final class NoPasscodeTests: XCTestCase {
    func credential(fullAccess: Bool) -> GatewayCredential {
        GatewayCredential(url: URL(string: "ws://127.0.0.1:9/")!, deviceId: UUID().uuidString.lowercased(), secret: Data(repeating: 1, count: 64), gatewayId: "g", name: "Mac", pairedAt: Date(), fullAccessAllowed: fullAccess)
    }

    /// Without a passcode the lock cannot be enforced: never lock the person
    /// out, refuse elevated operations instead, keep the rest usable.
    func testNoPasscodeNeverLocksAndRefusesElevatedOperations() {
        var passcode = false
        let lock = AppLock(defaults: UserDefaults(suiteName: "np-\(UUID().uuidString)")!, clock: SystemClock(), passcodeSet: { passcode }) { _ in true }
        let full = credential(fullAccess: true)
        let model = AppModel(credentials: InMemoryCredentialStore([full]), cache: InMemoryJournalCache(), lock: lock)
        let client = model.client(full.deviceId)!
        XCTAssertTrue(lock.required)
        XCTAssertFalse(lock.locked, "no passcode: never locked out")
        XCTAssertTrue(lock.requirementUnenforceable)
        XCTAssertEqual(model.elevatedRefusal(client, elevated: true), AppModel.noPasscodeMessage)
        XCTAssertNil(model.elevatedRefusal(client, elevated: false), "ordinary conversations stay usable")

        // A passcode set later (checked on foreground): the lock applies again.
        passcode = true
        lock.background()
        lock.foreground()
        XCTAssertTrue(lock.passcodeSet)
        XCTAssertNil(model.elevatedRefusal(client, elevated: true))
        model.remove(full.deviceId, tellGateway: false)
    }

    func testLimitedComputerIsNeverRefusedLocally() {
        let lock = AppLock(defaults: UserDefaults(suiteName: "np-\(UUID().uuidString)")!, clock: SystemClock(), passcodeSet: { false }) { _ in true }
        let limited = credential(fullAccess: false)
        let model = AppModel(credentials: InMemoryCredentialStore([limited]), cache: InMemoryJournalCache(), lock: lock)
        // The gateway answers accessTooHigh itself for a limited computer.
        XCTAssertNil(model.elevatedRefusal(model.client(limited.deviceId)!, elevated: true))
        model.remove(limited.deviceId, tellGateway: false)
    }

    /// Access set back to limited: the overlay goes without asking.
    func testLockClearsWhenTheRequirementLifts() async {
        let lock = AppLock(defaults: UserDefaults(suiteName: "np-\(UUID().uuidString)")!, clock: SystemClock(), passcodeSet: { true }) { _ in true }
        lock.setRequired(true, atLaunch: true)
        XCTAssertTrue(lock.locked)
        lock.setRequired(false, atLaunch: false)
        XCTAssertFalse(lock.locked)
    }
}

@MainActor
final class PairingRetryTests: XCTestCase {
    let link = PairLink(url: URL(string: "ws://192.168.0.190:17345/")!, deviceId: UUID().uuidString.lowercased(), pairSecret: Data(repeating: 2, count: 32), gatewayId: "g")

    /// The first LAN attempts fail while iOS shows the Local Network prompt;
    /// pairing retries the same link and succeeds.
    func testRetriesANetworkFailureBeforePairComplete() async throws {
        var attempts = 0
        var sawWaiting = false
        var model: AppModel!
        model = AppModel(credentials: InMemoryCredentialStore(), cache: InMemoryJournalCache()) { link, store in
            attempts += 1
            if attempts < 3 {
                if attempts == 2 { sawWaiting = model.waitingForLocalNetwork }
                throw GatewayError.closed(.closed(code: nil))
            }
            let secret = link.pairSecret + Data(repeating: 3, count: 32)
            try store(secret)
            return (nil, secret)
        }
        model.asksForNotifications = false
        model.pairingRetryDelay = { error, attempt, host in LocalNetwork.pairingRetryDelay(after: error, attempt: attempt, host: host).map { _ in 0.01 } }
        let client = try await model.pair(link, name: "Mac")
        XCTAssertEqual(attempts, 3)
        XCTAssertTrue(sawWaiting, "the sheet says it is waiting for Local Network permission")
        XCTAssertFalse(model.waitingForLocalNetwork)
        model.remove(client.id, tellGateway: false)
    }

    func testRefusalsAndFailuresAfterPairCompleteAreNotRetried() async {
        for (error, storesFirst) in [(GatewayError.closed(.unauthorized), false), (GatewayError.closed(.closed(code: nil)), true)] {
            var attempts = 0
            let model = AppModel(credentials: InMemoryCredentialStore(), cache: InMemoryJournalCache()) { link, store in
                attempts += 1
                if storesFirst { try store(link.pairSecret + Data(repeating: 3, count: 32)) }
                throw error
            }
            model.asksForNotifications = false
            model.pairingRetryDelay = { error, attempt, host in LocalNetwork.pairingRetryDelay(after: error, attempt: attempt, host: host).map { _ in 0.01 } }
            _ = try? await model.pair(link, name: "Mac")
            XCTAssertEqual(attempts, 1, "\(error) is not retried")
        }
    }
}
