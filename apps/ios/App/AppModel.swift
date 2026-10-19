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

    /// Shared with the notification extension: push keys only.
    static var keychainGroup: String { value("ChromeUseKeychainGroup") }
    /// The app's own group: full credentials.
    static var keychainAppGroup: String { value("ChromeUseKeychainAppGroup") }
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
    @ObservationIgnored private let pushKeys: PushKeyStore
    @ObservationIgnored private let cache: JournalCache
    @ObservationIgnored private let pairer: Pairer
    /// Off in tests, where the permission prompt would block.
    @ObservationIgnored var asksForNotifications = true

    /// Runs the pairing handshake; returns the open connection (nil in tests)
    /// and the combined secret, after calling `store` with it.
    typealias Pairer = (PairLink, @escaping (Data) throws -> Void) async throws -> (GatewayConnection?, Data)

    /// Live stores unless given: full credentials in the app-only keychain
    /// group, push keys in the group shared with the notification extension.
    init(credentials: CredentialStore? = nil, pushKeys: PushKeyStore? = nil,
         cache: JournalCache = FileJournalCache.applicationSupport(), pairer: Pairer? = nil, lock: AppLock? = nil) {
        let live = credentials == nil
        self.credentials = credentials ?? KeychainCredentialStore(accessGroup: AppConfig.keychainAppGroup)
        self.pushKeys = pushKeys ?? (live ? KeychainPushKeyStore(accessGroup: AppConfig.keychainGroup) : InMemoryPushKeyStore())
        self.cache = cache
        self.pairer = pairer ?? { link, store in
            let (connection, secret) = try await GatewayConnection.pair(link: link, store: store)
            return (connection, secret)
        }
        self.store = Store()
        self.push = PushManager()
        self.lock = lock ?? AppLock()
        push.onRelayToken = { [weak self] token in
            self?.clients.forEach { $0.relayToken = token }
        }
        load()
        // On launch, a computer that allows high-access conversations from
        // the phone requires the lock before anything shows.
        self.lock.setRequired(lockRequired, atLaunch: true)
    }

    /// Some paired computer lets this phone drive full-access conversations.
    var lockRequired: Bool {
        clients.contains { $0.credential.fullAccessAllowed }
    }

    func load() {
        do {
            for credential in try credentials.all() where client(credential.deviceId) == nil {
                let client = makeClient(credential)
                clients.append(client)
                client.start()
            }
        } catch {
            storeError = String(localized: "Cannot read the keychain: \(String(describing: error))")
        }
    }

    private func makeClient(_ credential: GatewayCredential) -> GatewayClient {
        let client = GatewayClient(credential: credential, cache: cache)
        client.relayToken = push.relayToken
        client.onPushTokenRejected = { [weak self] reason in self?.push.tokenRejected(reason: reason) }
        let keychain = credentials
        client.onCredentialChanged = { [weak self] credential in
            try? keychain.save(credential)
            if let self { self.lock.setRequired(self.lockRequired, atLaunch: false) }
        }
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

    /// A saved computer this link would replace: same device id, but another
    /// address or gateway. Pairing it needs an explicit confirmation.
    func conflict(for link: PairLink) -> GatewayCredential? {
        clients.map(\.credential).first { link.conflicts(with: $0) }
    }

    struct PairingConflict: Error {
        let saved: GatewayCredential
    }

    /// Pair, persist and connect. The combined secret is in the keychain
    /// before `pairComplete` leaves the phone. A link that reuses a saved
    /// computer's device id for another address or gateway is refused unless
    /// `replacing` confirms it.
    func pair(_ link: PairLink, name: String, replacing: Bool = false) async throws -> GatewayClient {
        let conflicting = conflict(for: link)
        if let conflicting, !replacing { throw PairingConflict(saved: conflicting) }
        let keychain = credentials
        let pushKeys = self.pushKeys
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
                let credential = credentialFor(secret)
                try keychain.save(credential)
                try pushKeys.save(PushKeyEntry(credential: credential))
                stored.value = true
            }
            let credential = credentialFor(secret)
            if let conflicting {
                // Confirmed: the old computer under this device id goes.
                removeCredential(conflicting)
            }
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
                // Only what this attempt stored; never a computer it was about to replace.
                let key = credentialFor(Data()).storageKey
                try? credentials.delete(storageKey: key)
                try? self.pushKeys.delete(storageKey: key)
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

    /// Drop one saved computer by its exact keychain key (the conflict case,
    /// where a new computer shares its device id).
    private func removeCredential(_ credential: GatewayCredential) {
        try? credentials.delete(storageKey: credential.storageKey)
        try? pushKeys.delete(storageKey: credential.storageKey)
        if let client = client(credential.deviceId), client.credential.storageKey == credential.storageKey {
            client.purge()
            clients.removeAll { $0 === client }
        }
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
        try? pushKeys.delete(deviceId: deviceId)
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
        lock.foreground()
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
