import Foundation

public enum GatewayError: Error, Equatable, Sendable {
    /// The connection ended (or never opened) for this reason.
    case closed(CloseReason)
    /// The gateway answered with `{error:{code,message}}`.
    case remote(code: String, message: String)
    /// No answer in time.
    case timeout
    /// Not connected right now.
    case notConnected
    /// The gateway broke the handshake or framing rules.
    case protocolViolation(String)
}

/// What a live connection delivers besides replies.
public enum ConnectionEvent: Sendable {
    /// A server event frame (`journal`, `subscriptionError`, `subscriptionEnded`).
    case server(JSONValue)
    case closed(CloseReason)
}

/// One authenticated, encrypted WebSocket to a gateway (spec "Transport and
/// encryption"): handshake, optional pairing rotation, then `{id, op, params}`
/// requests and server events, every frame sealed with strict sequence
/// numbers in both directions.
public actor GatewayConnection {
    public static let handshakeTimeout: TimeInterval = 10
    public static let maximumMessageSize = 4 * 1024 * 1024

    private let task: URLSessionWebSocketTask
    private var sender: FrameChannel
    private var receiver: FrameChannel
    private var nextId: Int64 = 1
    private var pending: [Int64: CheckedContinuation<JSONValue, Error>] = [:]
    private var closeReason: CloseReason?
    private var lastSent: Data?
    private let continuation: AsyncStream<ConnectionEvent>.Continuation
    public nonisolated let events: AsyncStream<ConnectionEvent>
    public nonisolated let gatewayId: String

    private init(task: URLSessionWebSocketTask, c2p: Data, p2c: Data, gatewayId: String) {
        self.task = task
        self.sender = FrameChannel(key: p2c)
        self.receiver = FrameChannel(key: c2p)
        self.gatewayId = gatewayId
        var continuation: AsyncStream<ConnectionEvent>.Continuation!
        self.events = AsyncStream(bufferingPolicy: .unbounded) { continuation = $0 }
        self.continuation = continuation
    }

    // MARK: Opening

    /// Connect as a paired device holding `secret` (`pair_secret ‖ phone_secret`).
    public static func connect(
        url: URL,
        deviceId: String,
        secret: Data,
        session: URLSession = .shared
    ) async throws -> GatewayConnection {
        let opened = try await handshake(url: url, deviceId: deviceId, ikm: secret, pairing: false, session: session)
        let connection = GatewayConnection(task: opened.task, c2p: opened.c2p, p2c: opened.p2c, gatewayId: opened.gatewayId)
        await connection.startReceiving()
        return connection
    }

    /// Pair with a scanned link: prove the pair secret, send a fresh
    /// `phoneSecret` in `pairComplete`, wait for the acknowledgement, then
    /// switch to the paired keys on the same connection. `store` is called
    /// with the combined secret before it is sent, so a crash after the
    /// gateway commits cannot lose it; if pairing then fails the caller
    /// discards what it stored.
    public static func pair(
        link: PairLink,
        phoneSecret: Data = MobileCrypto.random(MobileCrypto.secretLength),
        session: URLSession = .shared,
        store: (Data) throws -> Void
    ) async throws -> (connection: GatewayConnection, secret: Data) {
        let opened = try await handshake(url: link.url, deviceId: link.deviceId, ikm: link.pairSecret, pairing: true, session: session)
        var sender = FrameChannel(key: opened.p2c)
        var receiver = FrameChannel(key: opened.c2p)
        let combined = link.pairSecret + phoneSecret
        try store(combined)
        let request = Self.pairCompleteFrame(phoneSecret: phoneSecret)
        do {
            try await send(task: opened.task, .data(try sender.seal(request)))
            let reply = try await receive(task: opened.task, timeout: handshakeTimeout)
            guard case .data(let frame) = reply else {
                opened.task.cancel(with: .init(rawValue: 4400)!, reason: nil)
                throw GatewayError.protocolViolation("text frame after handshake")
            }
            let plain: Data
            do { plain = try receiver.open(frame) } catch {
                opened.task.cancel(with: .init(rawValue: 4400)!, reason: nil)
                throw GatewayError.protocolViolation("acknowledgement failed authentication")
            }
            let ack = try JSONValue.decode(plain)
            guard ack["id"] == .int(0), ack["result"]["paired"] == .bool(true) else {
                opened.task.cancel(with: .normalClosure, reason: nil)
                throw GatewayError.protocolViolation("unexpected pairing acknowledgement")
            }
        } catch let error as GatewayError {
            throw error
        } catch {
            throw GatewayError.closed(await closeReason(of: opened.task))
        }
        let paired = DeviceKeys(ikm: combined).session(gatewayNonce: opened.gatewayNonce, phoneNonce: opened.phoneNonce)
        let connection = GatewayConnection(task: opened.task, c2p: paired.c2p, p2c: paired.p2c, gatewayId: opened.gatewayId)
        await connection.startReceiving()
        return (connection, combined)
    }

    /// The first pairing frame, exactly as the spec's test vector spells it.
    public static func pairCompleteFrame(phoneSecret: Data) -> Data {
        Data(#"{"id":0,"op":"pairComplete","params":{"phoneSecret":"\#(Base64URL.encode(phoneSecret))"}}"#.utf8)
    }

    struct Opened {
        let task: URLSessionWebSocketTask
        let gatewayNonce: Data
        let phoneNonce: Data
        let c2p: Data
        let p2c: Data
        let gatewayId: String
    }

    static func handshake(url: URL, deviceId: String, ikm: Data, pairing: Bool, session: URLSession) async throws -> Opened {
        let task = session.webSocketTask(with: url)
        task.maximumMessageSize = maximumMessageSize
        task.resume()
        do {
            guard case .string(let text) = try await receive(task: task, timeout: handshakeTimeout) else {
                throw GatewayError.protocolViolation("challenge was not text")
            }
            let challenge = try JSONValue.decode(Data(text.utf8))
            guard challenge["type"] == "challenge", challenge["v"] == .int(1),
                  let nonceText = challenge["gatewayNonce"].string,
                  let gatewayNonce = Base64URL.decode(nonceText), gatewayNonce.count == MobileCrypto.nonceLength
            else {
                task.cancel(with: .init(rawValue: 4400)!, reason: nil)
                throw GatewayError.protocolViolation("malformed challenge")
            }
            let keys = DeviceKeys(ikm: ikm)
            let phoneNonce = MobileCrypto.random(MobileCrypto.nonceLength)
            let proof = keys.phoneProof(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce, deviceId: deviceId)
            let auth: JSONValue = [
                "type": "auth", "v": 1, "deviceId": .string(deviceId),
                "phoneNonce": .string(Base64URL.encode(phoneNonce)),
                "proof": .string(Base64URL.encode(proof)), "pairing": .bool(pairing),
            ]
            try await send(task: task, .string(auth.encodedString()))
            guard case .string(let readyText) = try await receive(task: task, timeout: handshakeTimeout) else {
                throw GatewayError.protocolViolation("ready was not text")
            }
            let ready = try JSONValue.decode(Data(readyText.utf8))
            guard ready["type"] == "ready", let proofText = ready["proof"].string,
                  let gatewayProof = Base64URL.decode(proofText),
                  keys.verifyGatewayProof(gatewayProof, gatewayNonce: gatewayNonce, phoneNonce: phoneNonce, deviceId: deviceId)
            else {
                task.cancel(with: .normalClosure, reason: nil)
                throw GatewayError.closed(.gatewayNotAuthentic)
            }
            let session = keys.session(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce)
            return Opened(
                task: task, gatewayNonce: gatewayNonce, phoneNonce: phoneNonce,
                c2p: session.c2p, p2c: session.p2c,
                gatewayId: challenge["gateway"].string ?? ""
            )
        } catch let error as GatewayError {
            if case .timeout = error { task.cancel(with: .normalClosure, reason: nil); throw GatewayError.closed(.handshakeTimeout) }
            throw error
        } catch {
            throw GatewayError.closed(await closeReason(of: task))
        }
    }

    // MARK: Requests

    /// Send `{id, op, params}` and wait for its reply's `result`.
    public func request(_ op: String, _ params: JSONValue = [:], timeout: TimeInterval = 60) async throws -> JSONValue {
        if let closeReason { throw GatewayError.closed(closeReason) }
        let id = nextId
        nextId += 1
        let frame: JSONValue = ["id": .int(id), "op": .string(op), "params": params]
        let sealed = try sender.seal(frame.encoded())
        lastSent = sealed
        let reply: JSONValue = try await withCheckedThrowingContinuation { continuation in
            pending[id] = continuation
            // Enqueued synchronously, so frames leave in sequence order.
            task.send(.data(sealed)) { error in
                if error != nil { Task { await self.failRequest(id, GatewayError.notConnected) } }
            }
            Task {
                try? await Task.sleep(nanoseconds: UInt64(timeout * 1_000_000_000))
                self.failRequest(id, GatewayError.timeout)
            }
        }
        if let error = reply["error"].object {
            throw GatewayError.remote(code: error["code"]?.string ?? "", message: error["message"]?.string ?? "")
        }
        return reply["result"]
    }

    private func failRequest(_ id: Int64, _ error: Error) {
        pending.removeValue(forKey: id)?.resume(throwing: error)
    }

    /// Test hook: send raw bytes as the next binary message.
    public func sendRaw(_ data: Data) async throws {
        try await Self.send(task: task, .data(data))
    }

    /// Test hook: send the last request frame again, as a replay would.
    public func replayLastFrame() async throws {
        guard let lastSent else { return }
        try await Self.send(task: task, .data(lastSent))
    }

    public func close() {
        task.cancel(with: .normalClosure, reason: nil)
        finish(.closed(code: 1000))
    }

    public var isOpen: Bool { closeReason == nil }

    public func ping() async -> Bool {
        await withCheckedContinuation { continuation in
            task.sendPing { error in continuation.resume(returning: error == nil) }
        }
    }

    // MARK: Receiving

    private func startReceiving() {
        let task = self.task
        Task.detached { [weak self] in
            while true {
                let message: URLSessionWebSocketTask.Message
                do {
                    message = try await task.receive()
                } catch {
                    let reason = await Self.closeReason(of: task)
                    await self?.finish(reason)
                    return
                }
                guard let self else { return }
                if await !self.handle(message) { return }
            }
        }
    }

    private func handle(_ message: URLSessionWebSocketTask.Message) -> Bool {
        guard case .data(let frame) = message else {
            return reject("text frame after handshake")
        }
        let value: JSONValue
        do {
            value = try JSONValue.decode(receiver.open(frame))
        } catch {
            return reject("frame rejected: \(error)")
        }
        if value["event"].string != nil {
            continuation.yield(.server(value))
        } else if let id = value["id"].int, let waiting = pending.removeValue(forKey: id) {
            waiting.resume(returning: value)
        }
        return true
    }

    private func reject(_ why: String) -> Bool {
        task.cancel(with: .init(rawValue: 4400)!, reason: Data(why.utf8))
        finish(.malformed)
        return false
    }

    private func finish(_ reason: CloseReason) {
        guard closeReason == nil else { return }
        closeReason = reason
        for (_, waiting) in pending { waiting.resume(throwing: GatewayError.closed(reason)) }
        pending.removeAll()
        continuation.yield(.closed(reason))
        continuation.finish()
    }

    // MARK: Socket helpers

    static func send(task: URLSessionWebSocketTask, _ message: URLSessionWebSocketTask.Message) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            task.send(message) { error in
                if let error { continuation.resume(throwing: error) } else { continuation.resume() }
            }
        }
    }

    static func receive(task: URLSessionWebSocketTask, timeout: TimeInterval) async throws -> URLSessionWebSocketTask.Message {
        try await withThrowingTaskGroup(of: URLSessionWebSocketTask.Message?.self) { group in
            group.addTask { try await task.receive() }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(timeout * 1_000_000_000))
                return nil
            }
            defer { group.cancelAll() }
            guard let first = try await group.next(), let message = first else {
                throw GatewayError.timeout
            }
            return message
        }
    }

    /// The close code arrives on the task shortly after `receive` fails.
    static func closeReason(of task: URLSessionWebSocketTask) async -> CloseReason {
        for _ in 0..<20 {
            let code = task.closeCode.rawValue
            if code != 0 { return CloseReason(closeCode: code) }
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
        return .closed(code: nil)
    }
}
