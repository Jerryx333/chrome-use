import CryptoKit
import Foundation

/// The message id of a send in flight. A retry after a timeout or a lost
/// reply reuses it, so the service (which deduplicates by id) delivers the
/// message once; a changed message gets a new id, since the service refuses
/// an id reused for different text.
public struct OutgoingMessage: Sendable {
    private var pending: (id: String, fingerprint: String)?

    public init() {}

    public mutating func id(text: String, images: [Data] = []) -> String {
        var hash = SHA256()
        hash.update(data: Data(text.utf8))
        for image in images { hash.update(data: Data(SHA256.hash(data: image))) }
        let fingerprint = Data(hash.finalize()).hex
        if let pending, pending.fingerprint == fingerprint { return pending.id }
        let id = UUID().uuidString.lowercased()
        pending = (id, fingerprint)
        return id
    }

    /// The service confirmed the message: the next one is new.
    public mutating func sent() {
        pending = nil
    }
}

extension PushRegistrar {
    /// A push Worker address the phone may use: https only, with a host.
    public static func workerURL(_ text: String) -> URL? {
        guard let url = URL(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              url.scheme?.lowercased() == "https", let host = url.host, !host.isEmpty
        else { return nil }
        return url
    }
}
