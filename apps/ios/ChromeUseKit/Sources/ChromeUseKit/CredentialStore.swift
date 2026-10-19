import Foundation
import Security

/// Where paired computers are kept. The app and the notification extension
/// share one keychain access group, so the extension can derive push keys.
public protocol CredentialStore: AnyObject, Sendable {
    func all() throws -> [GatewayCredential]
    func save(_ credential: GatewayCredential) throws
    func delete(deviceId: String) throws
}

public struct KeychainError: Error, Equatable {
    public let status: OSStatus
}

public final class KeychainCredentialStore: CredentialStore, @unchecked Sendable {
    let service: String
    let accessGroup: String?

    public init(service: String = "chrome-use.gateway", accessGroup: String?) {
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

    public func all() throws -> [GatewayCredential] {
        var query = base()
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
        let items = (result as? [Data]) ?? []
        let decoder = JSONDecoder()
        return items.compactMap { try? decoder.decode(GatewayCredential.self, from: $0) }
            .sorted { $0.pairedAt < $1.pairedAt }
    }

    public func save(_ credential: GatewayCredential) throws {
        let data = try JSONEncoder().encode(credential)
        var query = base()
        query[kSecAttrAccount as String] = credential.deviceId
        let update: [String: Any] = [kSecValueData as String: data]
        var status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            query[kSecValueData as String] = data
            // The notification extension runs while the phone is locked.
            query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            status = SecItemAdd(query as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw KeychainError(status: status) }
    }

    public func delete(deviceId: String) throws {
        var query = base()
        query[kSecAttrAccount as String] = deviceId
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError(status: status) }
    }
}

public final class InMemoryCredentialStore: CredentialStore, @unchecked Sendable {
    private var items: [String: GatewayCredential] = [:]
    private let lock = NSLock()

    public init(_ credentials: [GatewayCredential] = []) {
        for credential in credentials { items[credential.deviceId] = credential }
    }

    public func all() throws -> [GatewayCredential] {
        lock.withLock { items.values.sorted { $0.pairedAt < $1.pairedAt } }
    }

    public func save(_ credential: GatewayCredential) throws {
        lock.withLock { items[credential.deviceId] = credential }
    }

    public func delete(deviceId: String) throws {
        _ = lock.withLock { items.removeValue(forKey: deviceId) }
    }
}
