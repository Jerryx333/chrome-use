import Foundation

/// The decrypted body of a push (`{v, conversationId, kind, code, title, body}`).
public struct PushNotice: Equatable, Sendable {
    /// The stable reason the phone words the notification by; `body` is
    /// never matched on.
    public enum Code: String, Sendable {
        case needsInput, finished, stopped, failed, scheduleDone, scheduleFailed, test
    }

    public let conversationId: String
    public let rawKind: String
    /// nil for a code this build does not know: show a generic text.
    public let code: Code?
    public let title: String
    /// The turn's redacted preview, the schedule's result, or an English
    /// fallback; shown only next to the localized reason.
    public let body: String
    /// The paired computer whose push key opened it.
    public let deviceId: String

    /// Pick the key by the payload's kid; a payload naming no paired
    /// computer is dropped.
    public static func open(payload: String, credentials: [GatewayCredential]) -> PushNotice? {
        open(payload: payload, keys: credentials.map(PushKeyEntry.init))
    }

    /// What the notification extension calls: it holds push keys only.
    public static func open(payload: String, keys: [PushKeyEntry]) -> PushNotice? {
        guard let raw = Data(base64Encoded: payload), raw.count > 4,
              let entry = keys.first(where: { $0.kid == raw.prefix(4) }),
              let plain = PushCrypto.open(payload: payload, pushKey: entry.pushKey),
              let value = try? JSONValue.decode(plain), value["v"] == .int(1)
        else { return nil }
        return PushNotice(
            conversationId: value["conversationId"].string ?? "",
            rawKind: value["kind"].string ?? "",
            code: value["code"].string.flatMap(Code.init(rawValue:)),
            title: value["title"].string ?? "",
            body: value["body"].string ?? "",
            deviceId: entry.deviceId
        )
    }

    /// Keys the notification carries to the app for deep links.
    public var userInfo: [String: String] {
        ["deviceId": deviceId, "conversationId": conversationId, "kind": rawKind, "code": code?.rawValue ?? ""]
    }

    /// Whether the agent's own text (preview, schedule result) is worth
    /// showing under the localized reason.
    public var showsBody: Bool {
        switch code {
        case .finished, .stopped, .failed, .scheduleDone, .scheduleFailed: return !body.isEmpty
        default: return false
        }
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
