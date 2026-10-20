import Foundation

/// The journal events a phone has seen per conversation and the cursor to
/// resume from, so a reconnect or a cold start subscribes with `after` and
/// receives only what is new.
public protocol JournalCache: AnyObject, Sendable {
    func load(deviceId: String, conversationId: String) -> (events: [JSONValue], cursor: Int64)?
    func save(deviceId: String, conversationId: String, events: [JSONValue], cursor: Int64)
    func remove(deviceId: String, conversationId: String)
    func removeAll(deviceId: String)
}

public final class FileJournalCache: JournalCache, @unchecked Sendable {
    let directory: URL
    private let queue = DispatchQueue(label: "chrome-use.journal-cache")

    public init(directory: URL) {
        self.directory = directory
    }

    public static func applicationSupport() -> FileJournalCache {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return FileJournalCache(directory: base.appendingPathComponent("journals", isDirectory: true))
    }

    private func file(_ deviceId: String, _ conversationId: String) -> URL? {
        guard UUID(uuidString: deviceId) != nil, UUID(uuidString: conversationId) != nil else { return nil }
        return directory.appendingPathComponent(deviceId, isDirectory: true).appendingPathComponent("\(conversationId).json")
    }

    public func load(deviceId: String, conversationId: String) -> (events: [JSONValue], cursor: Int64)? {
        guard let url = file(deviceId, conversationId) else { return nil }
        return queue.sync {
            guard let data = try? Data(contentsOf: url), let value = try? JSONValue.decode(data),
                  let cursor = value["cursor"].int, let events = value["events"].array
            else { return nil }
            return (events, cursor)
        }
    }

    public func save(deviceId: String, conversationId: String, events: [JSONValue], cursor: Int64) {
        guard let url = file(deviceId, conversationId) else { return }
        let data = JSONValue.object(["cursor": .int(cursor), "events": .array(events)]).encoded()
        queue.async {
            try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            #if os(iOS)
            try? data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            #else
            try? data.write(to: url, options: [.atomic])
            #endif
        }
    }

    public func remove(deviceId: String, conversationId: String) {
        guard let url = file(deviceId, conversationId) else { return }
        queue.async { try? FileManager.default.removeItem(at: url) }
    }

    public func removeAll(deviceId: String) {
        guard UUID(uuidString: deviceId) != nil else { return }
        let url = directory.appendingPathComponent(deviceId, isDirectory: true)
        queue.async { try? FileManager.default.removeItem(at: url) }
    }
}

public final class InMemoryJournalCache: JournalCache, @unchecked Sendable {
    private var items: [String: ([JSONValue], Int64)] = [:]
    private let lock = NSLock()

    public init() {}

    public func load(deviceId: String, conversationId: String) -> (events: [JSONValue], cursor: Int64)? {
        lock.withLock { items["\(deviceId)/\(conversationId)"].map { ($0.0, $0.1) } }
    }

    public func save(deviceId: String, conversationId: String, events: [JSONValue], cursor: Int64) {
        lock.withLock { items["\(deviceId)/\(conversationId)"] = (events, cursor) }
    }

    public func remove(deviceId: String, conversationId: String) {
        _ = lock.withLock { items.removeValue(forKey: "\(deviceId)/\(conversationId)") }
    }

    public func removeAll(deviceId: String) {
        lock.withLock { items = items.filter { !$0.key.hasPrefix("\(deviceId)/") } }
    }
}
