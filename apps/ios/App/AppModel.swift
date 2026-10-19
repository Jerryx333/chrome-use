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

    init(credentials: CredentialStore? = nil, cache: JournalCache = FileJournalCache.applicationSupport()) {
        self.credentials = credentials ?? KeychainCredentialStore(accessGroup: AppConfig.keychainGroup)
        self.cache = cache
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
                let client = GatewayClient(credential: credential, cache: cache)
                client.relayToken = push.relayToken
                clients.append(client)
                client.start()
            }
        } catch {
            storeError = String(localized: "无法读取钥匙串：\(String(describing: error))")
        }
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
        let credentialFor = { (secret: Data) in
            GatewayCredential(url: link.url, deviceId: link.deviceId, secret: secret, gatewayId: link.gatewayId, name: name, pairedAt: pairedAt)
        }
        let stored = StoredFlag()
        do {
            let (connection, secret) = try await GatewayConnection.pair(link: link) { secret in
                try keychain.save(credentialFor(secret))
                stored.value = true
            }
            let credential = credentialFor(secret)
            if let replaced = repairing, replaced != link.deviceId { remove(replaced, tellGateway: false) }
            repairing = nil
            let client = GatewayClient(credential: credential, cache: cache)
            client.relayToken = push.relayToken
            clients.removeAll { $0.id == credential.deviceId }
            clients.append(client)
            client.adopt(connection)
            push.requestAuthorization()
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

    /// Writing (send, create, answer) needs the unlock or a running trial.
    func requireWrite() -> Bool {
        if store.entitlement.canWrite { return true }
        showPaywall = true
        return false
    }
}
