import ChromeUseKit
import Foundation
import Observation
import SwiftUI

private final class StoredFlag: @unchecked Sendable {
    var value = false
}

/// Values from Config/Identity.xcconfig, carried in Info.plist.
enum AppConfig {
    static func value(_ key: String) -> String {
        (Bundle.main.object(forInfoDictionaryKey: key) as? String) ?? ""
    }

    static var keychainGroup: String { value("ChromeUseKeychainGroup") }
    static var appGroup: String { value("ChromeUseAppGroup") }
    static var defaultPushWorker: URL { URL(string: value("ChromeUsePushWorkerURL")) ?? PushRegistrar.defaultBaseURL }
    static var unlockProduct: String { value("ChromeUseUnlockProduct") }
    static var trialProduct: String { value("ChromeUseTrialProduct") }
    static var version: String {
        "\(value("CFBundleShortVersionString")) (\(value("CFBundleVersion")))"
    }
}

enum Route: Hashable {
    case computer(String)
    case conversation(String, String)
    case schedules(String)
}

/// App state: paired computers and their live clients, navigation, and the
/// cross-cutting services (push, purchases, lock).
@MainActor
@Observable
final class AppModel {
    private(set) var clients: [GatewayClient] = []
    var path: [Route] = []
    /// A pairing link waiting for confirmation (scanned, pasted or opened).
    var pendingLink: PairLink?
    /// A credential whose pairing is being replaced (re-pair after revoke).
    var repairing: String?
    var showPairing = false
    var showSettings = false
    var showPaywall = false
    var storeError: String?

    let store: Store
    let push: PushManager
    let lock: AppLock
    @ObservationIgnored private let credentials: CredentialStore
    @ObservationIgnored private let cache: JournalCache
    @ObservationIgnored private let pairer: Pairer
    /// Off in tests, where the permission prompt would block.
    @ObservationIgnored var asksForNotifications = true

    /// Runs the pairing handshake; returns the open connection (nil in tests)
    /// and the combined secret, after calling `store` with it.
    typealias Pairer = (PairLink, @escaping (Data) throws -> Void) async throws -> (GatewayConnection?, Data)

    init(credentials: CredentialStore? = nil, cache: JournalCache = FileJournalCache.applicationSupport(), pairer: Pairer? = nil) {
        self.credentials = credentials ?? KeychainCredentialStore(accessGroup: AppConfig.keychainGroup)
        self.cache = cache
        self.pairer = pairer ?? { link, store in
            let (connection, secret) = try await GatewayConnection.pair(link: link, store: store)
            return (connection, secret)
        }
        self.store = Store()
        self.push = PushManager()
        self.lock = AppLock()
        push.onRelayToken = { [weak self] token in
            self?.clients.forEach { $0.relayToken = token }
        }
        load()
    }

    func load() {
        do {
            for credential in try credentials.all() where client(credential.deviceId) == nil {
                let client = makeClient(credential)
                clients.append(client)
                client.start()
            }
        } catch {
            storeError = String(localized: "无法读取钥匙串：\(String(describing: error))")
        }
    }

    private func makeClient(_ credential: GatewayCredential) -> GatewayClient {
        let client = GatewayClient(credential: credential, cache: cache)
        client.relayToken = push.relayToken
        client.onPushTokenRejected = { [weak self] reason in self?.push.tokenRejected(reason: reason) }
        let keychain = credentials
        client.onCredentialChanged = { credential in try? keychain.save(credential) }
        return client
    }

    func client(_ deviceId: String) -> GatewayClient? {
        clients.first { $0.id == deviceId }
    }

    // MARK: Pairing

    func open(url: URL) {
        guard url.scheme?.lowercased() == "chromeuse" else { return }
        if let link = try? PairLink(parsing: url.absoluteString) {
            pendingLink = link
            showPairing = true
        }
    }

    /// Pair, persist and connect. The combined secret is in the keychain
    /// before `pairComplete` leaves the phone.
    func pair(_ link: PairLink, name: String) async throws -> GatewayClient {
        let keychain = credentials
        let pairedAt = Date()
        // A name the user typed (or kept from the computer being re-paired)
        // wins over the gateway's; otherwise `hello` keeps it current.
        let replaced = repairing.flatMap(client)?.credential
        let custom = name != link.suggestedName || (replaced?.nameIsCustom == true && replaced?.name == name)
        let credentialFor = { (secret: Data) in
            GatewayCredential(url: link.url, deviceId: link.deviceId, secret: secret, gatewayId: link.gatewayId, name: name, pairedAt: pairedAt, nameIsCustom: custom)
        }
        let stored = StoredFlag()
        do {
            let (connection, secret) = try await pairer(link) { secret in
                try keychain.save(credentialFor(secret))
                stored.value = true
            }
            let credential = credentialFor(secret)
            if let replaced = repairing, replaced != link.deviceId { remove(replaced, tellGateway: false) }
            repairing = nil
            let client = makeClient(credential)
            clients.removeAll { $0.id == credential.deviceId }
            clients.append(client)
            if let connection { client.adopt(connection) } else { client.start() }
            if asksForNotifications { push.requestAuthorization() }
            return client
        } catch {
            if stored.value, (error as? GatewayError) != .closed(.unauthorized) {
                // pairComplete left the phone; the gateway may have committed
                // it before the connection dropped. Keep the credential: the
                // next connection either works or is refused with 4401.
                load()
            } else {
                try? credentials.delete(deviceId: link.deviceId)
            }
            throw error
        }
    }

    func rename(_ deviceId: String, to name: String) {
        guard let client = client(deviceId) else { return }
        var credential = client.credential
        credential.name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !credential.name.isEmpty else { return }
        credential.nameIsCustom = true
        try? credentials.save(credential)
        client.update(credential: credential)
    }

    /// Forget a computer. The gateway is told to drop the push token first,
    /// when it can be reached.
    func remove(_ deviceId: String, tellGateway: Bool = true) {
        guard let client = client(deviceId) else { return }
        if tellGateway, client.status == .connected {
            Task { _ = try? await client.call("pushUnregister") ; client.purge() }
        } else {
            client.purge()
        }
        try? credentials.delete(deviceId: deviceId)
        clients.removeAll { $0.id == deviceId }
        path.removeAll {
            switch $0 {
            case .computer(let id), .schedules(let id), .conversation(let id, _): return id == deviceId
            }
        }
    }

    func startRepair(_ deviceId: String) {
        repairing = deviceId
        pendingLink = nil
        showPairing = true
    }

    // MARK: Deep links from notifications

    /// The deep link a notification carries (`PushNotice.userInfo`, merged
    /// in by the notification extension).
    func openNotification(userInfo: [AnyHashable: Any]) {
        openNotification(deviceId: userInfo["deviceId"] as? String, conversationId: userInfo["conversationId"] as? String)
    }

    func openNotification(deviceId: String?, conversationId: String?) {
        guard let deviceId, client(deviceId) != nil else { return }
        if let conversationId, UUID(uuidString: conversationId) != nil {
            path = [.computer(deviceId), .conversation(deviceId, conversationId)]
        } else {
            path = [.computer(deviceId)]
        }
    }

    // MARK: Lifecycle

    func foreground() {
        clients.forEach { $0.reconnectNow() }
        Task { await store.refresh() }
        push.refreshIfNeeded()
    }

    /// Actions that let the agent act need the unlock or a running trial;
    /// reading, Stop and refusing a request never do.
    func allow(_ action: GatedAction) -> Bool {
        if !action.needsUnlock || store.entitlement.canWrite { return true }
        showPaywall = true
        return false
    }
}
