import Foundation

/// A parsed `chromeuse://pair?v=1&url=…&id=…&k=…&fp=…` link.
public struct PairLink: Equatable, Sendable {
    public let url: URL
    public let deviceId: String
    public let pairSecret: Data
    public let gatewayId: String
    /// The computer's display name (`n`), when the gateway sent one.
    public let computerName: String?

    public enum ParseError: Error, Equatable {
        case notAPairLink
        case unsupportedVersion
        case badURL
        case badDeviceId
        case badSecret
        case badGatewayId
        /// `ws://` to a host outside the tailnet / LAN; it needs `wss://`.
        case plaintextToPublicHost
    }

    public init(url: URL, deviceId: String, pairSecret: Data, gatewayId: String, computerName: String? = nil) {
        self.url = url
        self.deviceId = deviceId
        self.pairSecret = pairSecret
        self.gatewayId = gatewayId
        self.computerName = computerName
    }

    public init(parsing text: String) throws {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let components = URLComponents(string: trimmed),
              components.scheme?.lowercased() == "chromeuse",
              components.host?.lowercased() == "pair"
        else { throw ParseError.notAPairLink }
        var query: [String: String] = [:]
        for item in components.queryItems ?? [] where query[item.name] == nil {
            query[item.name] = item.value
        }
        guard query["v"] == "1" else { throw ParseError.unsupportedVersion }
        guard let raw = query["url"], let url = URL(string: raw),
              let scheme = url.scheme?.lowercased(), scheme == "ws" || scheme == "wss",
              url.host != nil
        else { throw ParseError.badURL }
        if scheme == "ws", !TransportPolicy.allowsPlaintext(host: url.host ?? "") {
            throw ParseError.plaintextToPublicHost
        }
        guard let id = query["id"], UUID(uuidString: id) != nil else { throw ParseError.badDeviceId }
        guard let k = query["k"], let secret = Base64URL.decode(k), secret.count == MobileCrypto.secretLength
        else { throw ParseError.badSecret }
        guard let fp = query["fp"], let identity = Base64URL.decode(fp), identity.count == 16
        else { throw ParseError.badGatewayId }
        let name = query["n"]?.trimmingCharacters(in: .whitespacesAndNewlines)
        self.init(url: url, deviceId: id, pairSecret: secret, gatewayId: fp, computerName: name?.isEmpty == false ? String(name!.prefix(80)) : nil)
    }

    /// A name for the computer until the user renames it: the gateway's,
    /// else the URL's host.
    /// A link for this device id that points at a different computer than
    /// the one saved under it: pairing would replace that computer.
    public func conflicts(with saved: GatewayCredential) -> Bool {
        saved.deviceId == deviceId && (saved.url != url || saved.gatewayId != gatewayId)
    }

    public var suggestedName: String {
        computerName ?? url.host ?? "chrome-use"
    }
}

/// What the phone keeps for one paired computer. `secret` is
/// `pair_secret ‖ phone_secret` (64 bytes); every long-term key derives
/// from it.
public struct GatewayCredential: Codable, Identifiable, Hashable, Sendable {
    public var id: String { deviceId }
    public var url: URL
    public var deviceId: String
    public var secret: Data
    public var gatewayId: String
    public var name: String
    public var pairedAt: Date
    /// The user typed `name`; the gateway's name (`hello`) no longer replaces it.
    public var nameIsCustom: Bool
    /// The chrome-use version the gateway reported in `hello`.
    public var gatewayVersion: String?
    /// The gateway lets this phone drive conversations with full access or
    /// command execution (`hello.fullAccessAllowed`); the app then requires
    /// its lock.
    public var fullAccessAllowed: Bool

    public init(url: URL, deviceId: String, secret: Data, gatewayId: String, name: String, pairedAt: Date, nameIsCustom: Bool = false, gatewayVersion: String? = nil, fullAccessAllowed: Bool = false) {
        self.url = url
        self.deviceId = deviceId
        self.secret = secret
        self.gatewayId = gatewayId
        self.name = name
        self.pairedAt = pairedAt
        self.nameIsCustom = nameIsCustom
        self.gatewayVersion = gatewayVersion
        self.fullAccessAllowed = fullAccessAllowed
    }

    enum CodingKeys: String, CodingKey {
        case url, deviceId, secret, gatewayId, name, pairedAt, nameIsCustom, gatewayVersion, fullAccessAllowed
    }

    /// Credentials stored by earlier builds lack the newer fields.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            url: try c.decode(URL.self, forKey: .url),
            deviceId: try c.decode(String.self, forKey: .deviceId),
            secret: try c.decode(Data.self, forKey: .secret),
            gatewayId: try c.decode(String.self, forKey: .gatewayId),
            name: try c.decode(String.self, forKey: .name),
            pairedAt: try c.decode(Date.self, forKey: .pairedAt),
            nameIsCustom: try c.decodeIfPresent(Bool.self, forKey: .nameIsCustom) ?? false,
            gatewayVersion: try c.decodeIfPresent(String.self, forKey: .gatewayVersion),
            fullAccessAllowed: try c.decodeIfPresent(Bool.self, forKey: .fullAccessAllowed) ?? false
        )
    }

    public var keys: DeviceKeys { DeviceKeys(ikm: secret) }
}

/// Why a connection ended, from its WebSocket close code (spec,
/// "Transport and encryption").
public enum CloseReason: Equatable, Sendable {
    /// 4400: a frame or handshake message was rejected.
    case malformed
    /// 4401: unknown device, wrong key, wrong mode, revoked or disabled.
    case unauthorized
    /// 4403: revoked or disabled while connected.
    case revoked
    /// 4408: no reply within the handshake limit.
    case handshakeTimeout
    /// 4409: paired to another profile than this gateway serves.
    case wrongProfile
    /// 4429: too many failed handshakes; back off.
    case backoff
    /// The gateway's proof was wrong: whoever answered is not the paired computer.
    case gatewayNotAuthentic
    /// Closed normally, or the network dropped.
    case closed(code: Int?)

    public init(closeCode: Int?) {
        switch closeCode {
        case 4400: self = .malformed
        case 4401: self = .unauthorized
        case 4403: self = .revoked
        case 4408: self = .handshakeTimeout
        case 4409: self = .wrongProfile
        case 4429: self = .backoff
        default: self = .closed(code: closeCode)
        }
    }

    /// Reconnecting cannot help: the user has to act (re-pair, other gateway).
    public var isPermanent: Bool {
        switch self {
        case .unauthorized, .revoked, .wrongProfile, .gatewayNotAuthentic: return true
        default: return false
        }
    }

    /// The device record is gone or disabled: offer pairing again.
    public var offersRepair: Bool {
        self == .unauthorized || self == .revoked
    }

    /// Delay before reconnect attempt `attempt` (0-based), or nil for none.
    public func retryDelay(attempt: Int) -> TimeInterval? {
        if isPermanent { return nil }
        let base: TimeInterval = self == .backoff ? 5 : 1
        let cap: TimeInterval = self == .backoff ? 300 : 30
        return min(cap, base * pow(2, Double(min(attempt, 10))))
    }
}
