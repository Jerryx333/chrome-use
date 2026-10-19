import Foundation

/// The decrypted body of a push (`{v, conversationId, kind, title, body}`).
public struct PushNotice: Equatable, Sendable {
    public enum Kind: String, Sendable {
        case needsInput, turnEnded, scheduleDone, scheduleFailed, test
    }

    public let conversationId: String
    /// Unknown kinds are treated like `test`: show title/body, open the app.
    public let kind: Kind
    public let rawKind: String
    /// Stable reason to localize by (`needsInput`, `finished`, `stopped`,
    /// `failed`, `scheduleDone`, `scheduleFailed`, `test`); absent from
    /// older gateways.
    public let code: String?
    public let title: String
    public let body: String
    /// The paired computer whose push key opened it.
    public let deviceId: String

    public static func open(payload: String, credentials: [GatewayCredential]) -> PushNotice? {
        // The kid prefix names the key; without a match (older gateways),
        // the key that authenticates the payload identifies the computer.
        let named = credentials.filter { PushCrypto.names(payload: payload, pushKey: $0.keys.push) }
        for credential in named + credentials.filter({ c in !named.contains(c) }) {
            guard let plain = PushCrypto.open(payload: payload, pushKey: credential.keys.push),
                  let value = try? JSONValue.decode(plain), value["v"] == .int(1)
            else { continue }
            let raw = value["kind"].string ?? "test"
            return PushNotice(
                conversationId: value["conversationId"].string ?? "",
                kind: Kind(rawValue: raw) ?? .test,
                rawKind: raw,
                code: value["code"].string,
                title: value["title"].string ?? "",
                body: value["body"].string ?? "",
                deviceId: credential.deviceId
            )
        }
        return nil
    }

    /// Bodies the phone says in its own language. The gateway's body is
    /// English boilerplate for these, or an agent summary (kept as is).
    public enum StandardBody: Equatable, Sendable {
        case waitingForApproval, finished, stopped, failed, test
    }

    public var standardBody: StandardBody? {
        let code = self.code ?? (kind == .needsInput ? "needsInput" : nil)
        switch code {
        case "needsInput": return .waitingForApproval
        case "test": return .test
        case "finished" where body == "Finished" || body.isEmpty: return .finished
        case "stopped" where body == "Stopped" || body.isEmpty: return .stopped
        case "failed" where body == "Failed" || body.isEmpty: return .failed
        default:
            // Older gateways: no code, recognise the fixed English bodies.
            guard self.code == nil, kind == .turnEnded else { return nil }
            return ["Finished": .finished, "Stopped": .stopped, "Failed": .failed][body]
        }
    }

    /// Keys the notification carries to the app for deep links.
    public var userInfo: [String: String] {
        ["deviceId": deviceId, "conversationId": conversationId, "kind": rawKind]
    }
}

public struct RelayRegistration: Codable, Equatable, Sendable {
    public let relayToken: String
    public let expiresAt: Date
    public let deviceToken: String

    /// Refresh a week before the Worker's 90-day expiry, or when the APNs
    /// token changed.
    public func needsRefresh(deviceToken: String, clock: Clock) -> Bool {
        deviceToken != self.deviceToken || clock.now.addingTimeInterval(7 * 86_400) >= expiresAt
    }
}

/// Client of the push Worker's `POST /v1/register`.
public struct PushRegistrar: Sendable {
    public static let defaultBaseURL = URL(string: "https://chrome-use-push.leeguoo.com")!
    public let baseURL: URL
    public let session: URLSession

    public init(baseURL: URL = defaultBaseURL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    public enum Failure: Error, Equatable {
        case http(Int)
        case malformedReply
    }

    public func register(deviceToken: String, sandbox: Bool) async throws -> RelayRegistration {
        var request = URLRequest(url: baseURL.appendingPathComponent("v1/register"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        let body: JSONValue = ["platform": "ios", "deviceToken": .string(deviceToken), "env": sandbox ? "sandbox" : "production"]
        request.httpBody = body.encoded()
        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard status == 200 else { throw Failure.http(status) }
        let reply = try JSONValue.decode(data)
        guard let token = reply["relayToken"].string else { throw Failure.malformedReply }
        return RelayRegistration(relayToken: token, expiresAt: Self.date(reply["expiresAt"]) ?? Date().addingTimeInterval(80 * 86_400), deviceToken: deviceToken)
    }

    /// `expiresAt` as unix seconds, milliseconds or an ISO 8601 string.
    static func date(_ value: JSONValue) -> Date? {
        if let number = value.double {
            return Date(timeIntervalSince1970: number > 1e12 ? number / 1000 : number)
        }
        if let text = value.string {
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            return formatter.date(from: text) ?? ISO8601DateFormatter().date(from: text)
        }
        return nil
    }
}
