import Foundation
import Observation

/// One paired computer as the app sees it: keeps a connection up with
/// backoff, re-registers push and re-subscribes open conversations from
/// their stored cursors on every connection, and folds journal events into
/// timelines.
@MainActor
@Observable
public final class GatewayClient: Identifiable {
    public enum Status: Equatable, Sendable {
        case idle
        case connecting
        case connected
        case retrying(at: Date, reason: CloseReason)
        case failed(CloseReason)
    }

    public nonisolated let id: String
    public private(set) var credential: GatewayCredential
    public private(set) var status: Status = .idle
    public private(set) var conversations: [ConversationSummary] = []
    public private(set) var timelines: [String: Timeline] = [:]
    public private(set) var summaries: [String: ConversationSummary] = [:]
    public private(set) var queuedMessages: [String: [JSONValue]] = [:]
    public private(set) var subscriptionErrors: [String: String] = [:]
    /// Conversations that no longer exist (`subscriptionEnded`).
    public private(set) var ended: Set<String> = []

    /// The push relay token; sent with `pushRegister` on every connection.
    public var relayToken: String? {
        didSet {
            guard relayToken != oldValue, let connection else { return }
            Task { await self.registerPush(on: connection) }
        }
    }

    @ObservationIgnored private var connection: GatewayConnection?
    @ObservationIgnored private var runner: Task<Void, Never>?
    @ObservationIgnored private var subscribed: Set<String> = []
    @ObservationIgnored private var journals: [String: [JSONValue]] = [:]
    @ObservationIgnored private let cache: JournalCache
    @ObservationIgnored private let session: URLSession
    @ObservationIgnored private var retryNow: CheckedContinuation<Void, Never>?

    public init(credential: GatewayCredential, cache: JournalCache, session: URLSession = .shared) {
        self.id = credential.deviceId
        self.credential = credential
        self.cache = cache
        self.session = session
    }

    public func update(credential: GatewayCredential) {
        self.credential = credential
    }

    // MARK: Lifecycle

    public func start() {
        guard runner == nil else { return }
        runner = Task { await self.run() }
    }

    public func stop() {
        runner?.cancel()
        runner = nil
        retryNow?.resume()
        retryNow = nil
        let connection = self.connection
        self.connection = nil
        Task { await connection?.close() }
        status = .idle
    }

    /// Skip the backoff wait (pull to refresh, app foregrounded).
    public func reconnectNow() {
        if case .failed = status { stop(); start(); return }
        if runner == nil { start(); return }
        retryNow?.resume()
        retryNow = nil
    }

    /// Adopt a connection opened by pairing, then keep it up like any other.
    public func adopt(_ connection: GatewayConnection) {
        runner?.cancel()
        runner = Task { await self.run(first: connection) }
    }

    private func run(first: GatewayConnection? = nil) async {
        var attempt = 0
        var pending = first
        while !Task.isCancelled {
            let reason: CloseReason
            do {
                let live: GatewayConnection
                if let opened = pending {
                    live = opened
                    pending = nil
                } else {
                    status = .connecting
                    live = try await GatewayConnection.connect(
                        url: credential.url, deviceId: credential.deviceId, secret: credential.secret, session: session
                    )
                }
                if Task.isCancelled { await live.close(); return }
                connection = live
                status = .connected
                attempt = 0
                await afterConnect(live)
                reason = await pump(live)
                connection = nil
            } catch GatewayError.closed(let why) {
                reason = why
            } catch {
                reason = .closed(code: nil)
            }
            if Task.isCancelled { return }
            guard let delay = reason.retryDelay(attempt: attempt) else {
                status = .failed(reason)
                runner = nil
                return
            }
            status = .retrying(at: Date().addingTimeInterval(delay), reason: reason)
            attempt += 1
            await waitOrRetryNow(delay)
        }
    }

    private func waitOrRetryNow(_ delay: TimeInterval) async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            retryNow = continuation
            Task {
                try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
                if let waiting = self.retryNow {
                    self.retryNow = nil
                    waiting.resume()
                }
            }
        }
    }

    private func pump(_ live: GatewayConnection) async -> CloseReason {
        let keepAlive = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 30_000_000_000)
                if Task.isCancelled { return }
                if await !live.ping() { await live.close() }
            }
        }
        defer { keepAlive.cancel() }
        for await event in live.events {
            switch event {
            case .server(let frame): handle(frame)
            case .closed(let reason): return reason
            }
        }
        return .closed(code: nil)
    }

    private func afterConnect(_ live: GatewayConnection) async {
        await registerPush(on: live)
        for conversationId in subscribed {
            await sendSubscribe(conversationId, on: live)
        }
        try? await refreshConversations()
    }

    private func registerPush(on live: GatewayConnection) async {
        guard let relayToken else { return }
        _ = try? await live.request("pushRegister", ["relayToken": .string(relayToken)])
    }

    // MARK: Requests

    /// A request on the current connection, waiting briefly for one to open.
    public func call(_ op: String, _ params: JSONValue = [:], timeout: TimeInterval = 60) async throws -> JSONValue {
        let deadline = Date().addingTimeInterval(10)
        while connection == nil {
            if case .failed(let reason) = status { throw GatewayError.closed(reason) }
            if Date() > deadline { throw GatewayError.notConnected }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        return try await connection!.request(op, params, timeout: timeout)
    }

    @discardableResult
    public func refreshConversations() async throws -> [ConversationSummary] {
        let list = ConversationSummary.list(try await call("conversationList"))
        conversations = list
        for summary in list { summaries[summary.id] = summary }
        return list
    }

    public func discover() async throws -> [Agent] {
        Agent.list(try await call("discover"))
    }

    public func schedules() async throws -> [Schedule] {
        Schedule.list(try await call("scheduleList"))
    }

    public func runSchedule(_ id: String) async throws {
        _ = try await call("scheduleRun", ["id": .string(id)])
    }

    public func setSchedule(_ id: String, enabled: Bool) async throws {
        _ = try await call("scheduleSave", ["id": .string(id), "enabled": .bool(enabled)])
    }

    /// Create a conversation. Tool access is capped at `writeFiles` and full
    /// access is never asked for (the gateway refuses both anyway).
    public func create(agentId: String, model: String?, toolAccess: ToolAccess?) async throws -> ConversationSummary {
        var params: [String: JSONValue] = ["agentId": .string(agentId), "fullAccess": false]
        if let model, !model.isEmpty { params["model"] = .string(model) }
        if let toolAccess { params["options"] = ["toolAccess": .string(toolAccess.rawValue)] }
        let summary = ConversationSummary(try await call("conversationCreate", .object(params))["conversation"])
        summaries[summary.id] = summary
        conversations.insert(summary, at: 0)
        return summary
    }

    /// Upload prepared JPEGs, then send. `messageId` is fresh per message.
    public func send(conversationId: String, text: String, images: [Data] = [], messageId: String = UUID().uuidString.lowercased()) async throws {
        var attachments: [JSONValue] = []
        for image in images.prefix(ImagePreparer.maximumImages) {
            let put = try await call("conversationAttachmentPut", ["conversationId": .string(conversationId), "data": .string(image.base64EncodedString())])
            attachments.append(put["attachment"])
        }
        var params: [String: JSONValue] = ["conversationId": .string(conversationId), "messageId": .string(messageId), "text": .string(text)]
        if !attachments.isEmpty { params["attachments"] = .array(attachments) }
        let result = try await call("conversationSend", .object(params))
        if !result["conversation"].isNull { summaries[conversationId] = ConversationSummary(result["conversation"]) }
    }

    public func stopTurn(conversationId: String) async throws {
        _ = try await call("conversationStop", ["conversationId": .string(conversationId)])
    }

    public func respond(conversationId: String, request: PendingRequest, decision: JSONValue) async throws {
        _ = try await call("conversationRespond", ApprovalForm.respondParams(conversationId: conversationId, request: request, decision: decision))
    }

    public func delete(conversationId: String) async throws {
        _ = try await call("conversationDelete", ["conversationId": .string(conversationId), "confirm": true])
        unsubscribe(conversationId)
        conversations.removeAll { $0.id == conversationId }
        timelines.removeValue(forKey: conversationId)
        journals.removeValue(forKey: conversationId)
        cache.remove(deviceId: credential.deviceId, conversationId: conversationId)
    }

    public struct Screenshot: Sendable {
        public let jpeg: Data
        public let width: Int
        public let height: Int
    }

    public func screenshot(conversationId: String) async throws -> Screenshot {
        let result = try await call("screenshot", ["conversationId": .string(conversationId)])
        guard let data = result["data"].string.flatMap({ Data(base64Encoded: $0) }) else {
            throw GatewayError.protocolViolation("screenshot without data")
        }
        return Screenshot(jpeg: data, width: Int(result["width"].int ?? 0), height: Int(result["height"].int ?? 0))
    }

    // MARK: Subscriptions

    /// Follow a conversation: cached events show at once, the gateway sends
    /// what came after the cached cursor.
    public func subscribe(_ conversationId: String) {
        if timelines[conversationId] == nil { restore(conversationId) }
        subscribed.insert(conversationId)
        if let connection { Task { await self.sendSubscribe(conversationId, on: connection) } }
    }

    public func unsubscribe(_ conversationId: String) {
        guard subscribed.remove(conversationId) != nil, let connection else { return }
        Task { _ = try? await connection.request("unsubscribe", ["conversationId": .string(conversationId)]) }
    }

    public func cursor(of conversationId: String) -> Int64 {
        timelines[conversationId]?.cursor ?? 0
    }

    private func restore(_ conversationId: String) {
        var timeline = Timeline()
        if let cached = cache.load(deviceId: credential.deviceId, conversationId: conversationId),
           (try? timeline.apply(events: cached.events, cursor: cached.cursor)) != nil {
            journals[conversationId] = cached.events
        } else {
            timeline = Timeline()
            journals[conversationId] = []
        }
        timelines[conversationId] = timeline
    }

    private func sendSubscribe(_ conversationId: String, on live: GatewayConnection) async {
        let after = timelines[conversationId]?.cursor ?? 0
        do {
            _ = try await live.request("subscribe", ["conversationId": .string(conversationId), "after": .int(after)])
            subscriptionErrors.removeValue(forKey: conversationId)
        } catch GatewayError.remote(_, let message) {
            subscriptionErrors[conversationId] = message
        } catch {}
    }

    /// Apply a server event frame; exposed for tests.
    public func handle(_ frame: JSONValue) {
        guard let conversationId = frame["conversationId"].string else { return }
        switch frame["event"].string {
        case "journal":
            guard subscribed.contains(conversationId) else { return }
            var timeline = timelines[conversationId] ?? Timeline()
            let events = frame["events"].array ?? []
            do {
                try timeline.apply(events: events, cursor: frame["cursor"].int ?? timeline.cursor)
            } catch {
                // The cache and the journal disagree: start over from the beginning.
                timelines[conversationId] = Timeline()
                journals[conversationId] = []
                cache.remove(deviceId: credential.deviceId, conversationId: conversationId)
                if let connection { Task { await self.sendSubscribe(conversationId, on: connection) } }
                return
            }
            let previous = timelines[conversationId]?.cursor ?? 0
            timelines[conversationId] = timeline
            var journal = journals[conversationId] ?? []
            journal.append(contentsOf: events.filter { ($0["sequence"].int ?? 0) > previous })
            journals[conversationId] = journal
            cache.save(deviceId: credential.deviceId, conversationId: conversationId, events: journal, cursor: timeline.cursor)
            if !frame["conversation"].isNull {
                let summary = ConversationSummary(frame["conversation"])
                summaries[conversationId] = summary
                if let at = conversations.firstIndex(where: { $0.id == conversationId }) { conversations[at] = summary }
            }
            queuedMessages[conversationId] = frame["queuedMessages"].array ?? []
            subscriptionErrors.removeValue(forKey: conversationId)
        case "subscriptionError":
            subscriptionErrors[conversationId] = frame["error"].string ?? frame["error"].encodedString()
        case "subscriptionEnded":
            subscribed.remove(conversationId)
            ended.insert(conversationId)
            conversations.removeAll { $0.id == conversationId }
        default:
            break
        }
    }

    /// Forget everything kept for this computer (it was removed).
    public func purge() {
        stop()
        cache.removeAll(deviceId: credential.deviceId)
        timelines.removeAll()
        journals.removeAll()
    }
}
