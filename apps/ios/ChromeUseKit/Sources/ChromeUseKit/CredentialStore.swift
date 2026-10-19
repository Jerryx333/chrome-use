import Foundation
import Security

/// Where paired computers are kept: the full credential (it holds
/// `pair_secret ‖ phone_secret`) in an app-only keychain group. The
/// notification extension gets only the push keys (`PushKeyStore`).
public protocol CredentialStore: AnyObject, Sendable {
    func all() throws -> [GatewayCredential]
    func save(_ credential: GatewayCredential) throws
    func delete(deviceId: String) throws
    /// Exactly one computer's item (`GatewayCredential.storageKey`).
    func delete(storageKey: String) throws
}

/// What the notification extension may read: per computer, the push key
/// and its key id, never the secret the other keys derive from.
public struct PushKeyEntry: Codable, Hashable, Sendable {
    public let deviceId: String
    public let gatewayId: String
    public let kid: Data
    public let pushKey: Data

    public init(credential: GatewayCredential) {
        let push = credential.keys.push
        self.deviceId = credential.deviceId
        self.gatewayId = credential.gatewayId
        self.kid = PushCrypto.kid(pushKey: push)
        self.pushKey = push
    }
}

public protocol PushKeyStore: AnyObject, Sendable {
    func all() throws -> [PushKeyEntry]
    func save(_ entry: PushKeyEntry) throws
    func delete(deviceId: String) throws
    func delete(storageKey: String) throws
}

public struct KeychainError: Error, Equatable {
    public let status: OSStatus
}

extension GatewayCredential {
    /// Keychain account: a pair link from another computer that reuses a
    /// device id cannot overwrite this computer's item.
    public var storageKey: String { "\(gatewayId)/\(deviceId)" }
}

/// Generic-password items of one service in one access group.
public final class KeychainItems: @unchecked Sendable {
    public let service: String
    public let accessGroup: String?

    public init(service: String, accessGroup: String?) {
        self.service = service
        self.accessGroup = accessGroup
    }

    private func base() -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecUseDataProtectionKeychain as String: true,
        ]
        if let accessGroup, !accessGroup.isEmpty { query[kSecAttrAccessGroup as String] = accessGroup }
        return query
    }

    /// (account, data, access group) of every item.
    public func all() throws -> [(account: String, data: Data, group: String?)] {
        var query = base()
        query[kSecReturnData as String] = true
        query[kSecReturnAttributes as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
        return ((result as? [[String: Any]]) ?? []).compactMap { item in
            guard let account = item[kSecAttrAccount as String] as? String,
                  let data = item[kSecValueData as String] as? Data else { return nil }
            return (account, data, item[kSecAttrAccessGroup as String] as? String)
        }
    }

    public func put(account: String, data: Data) throws {
        var query = base()
        query[kSecAttrAccount as String] = account
        var status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            query[kSecValueData as String] = data
            // The notification extension runs while the phone is locked.
            query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            status = SecItemAdd(query as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
    }

    public func delete(account: String) throws {
        var query = base()
        query[kSecAttrAccount as String] = account
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError(status: status) }
    }
}

public final class KeychainCredentialStore: CredentialStore, @unchecked Sendable {
    public let items: KeychainItems

    /// `accessGroup` is the app-only group; the shared one is for push keys.
    public init(service: String = "chrome-use.computer", accessGroup: String?) {
        items = KeychainItems(service: service, accessGroup: accessGroup)
    }

    public func all() throws -> [GatewayCredential] {
        let decoder = JSONDecoder()
        return try items.all().compactMap { try? decoder.decode(GatewayCredential.self, from: $0.data) }
            .sorted { $0.pairedAt < $1.pairedAt }
    }

    public func save(_ credential: GatewayCredential) throws {
        try items.put(account: credential.storageKey, data: try JSONEncoder().encode(credential))
    }

    public func delete(storageKey: String) throws {
        try items.delete(account: storageKey)
    }

    /// Every item of this device id, whatever its account spelling.
    public func delete(deviceId: String) throws {
        let decoder = JSONDecoder()
        for item in try items.all() {
            if (try? decoder.decode(GatewayCredential.self, from: item.data))?.deviceId == deviceId || item.account == deviceId {
                try items.delete(account: item.account)
            }
        }
    }
}

public final class KeychainPushKeyStore: PushKeyStore, @unchecked Sendable {
    public let items: KeychainItems

    /// `accessGroup` is the group shared with the notification extension.
    public init(service: String = "chrome-use.pushkey", accessGroup: String?) {
        items = KeychainItems(service: service, accessGroup: accessGroup)
    }

    public func all() throws -> [PushKeyEntry] {
        let decoder = JSONDecoder()
        return try items.all().compactMap { try? decoder.decode(PushKeyEntry.self, from: $0.data) }
    }

    public func save(_ entry: PushKeyEntry) throws {
        try items.put(account: "\(entry.gatewayId)/\(entry.deviceId)", data: try JSONEncoder().encode(entry))
    }

    public func delete(storageKey: String) throws {
        try items.delete(account: storageKey)
    }

    public func delete(deviceId: String) throws {
        let decoder = JSONDecoder()
        for item in try items.all() where (try? decoder.decode(PushKeyEntry.self, from: item.data))?.deviceId == deviceId {
            try items.delete(account: item.account)
        }
    }
}

/// Moves computers saved by earlier builds (full credential in the shared
/// group, which the extension could read) into the app-only store plus a
/// push-key entry, then deletes the shared copy.
public enum CredentialMigration {
    @discardableResult
    public static func run(legacy: CredentialStore, into credentials: CredentialStore, pushKeys: PushKeyStore) throws -> Int {
        let old = try legacy.all()
        for credential in old {
            try credentials.save(credential)
            try pushKeys.save(PushKeyEntry(credential: credential))
            try legacy.delete(deviceId: credential.deviceId)
        }
        return old.count
    }
}

public final class InMemoryCredentialStore: CredentialStore, @unchecked Sendable {
    private var items: [String: GatewayCredential] = [:]
    private let lock = NSLock()

    public init(_ credentials: [GatewayCredential] = []) {
        for credential in credentials { items[credential.storageKey] = credential }
    }

    public func all() throws -> [GatewayCredential] {
        lock.withLock { items.values.sorted { $0.pairedAt < $1.pairedAt } }
    }

    public func save(_ credential: GatewayCredential) throws {
        lock.withLock { items[credential.storageKey] = credential }
    }

    public func delete(deviceId: String) throws {
        lock.withLock { items = items.filter { $0.value.deviceId != deviceId } }
    }

    public func delete(storageKey: String) throws {
        _ = lock.withLock { items.removeValue(forKey: storageKey) }
    }
}

public final class InMemoryPushKeyStore: PushKeyStore, @unchecked Sendable {
    private var items: [String: PushKeyEntry] = [:]
    private let lock = NSLock()

    public init() {}

    public func all() throws -> [PushKeyEntry] {
        lock.withLock { Array(items.values) }
    }

    public func save(_ entry: PushKeyEntry) throws {
        lock.withLock { items["\(entry.gatewayId)/\(entry.deviceId)"] = entry }
    }

    public func delete(deviceId: String) throws {
        lock.withLock { items = items.filter { $0.value.deviceId != deviceId } }
    }

    public func delete(storageKey: String) throws {
        _ = lock.withLock { items.removeValue(forKey: storageKey) }
    }
}
