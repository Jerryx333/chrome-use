import CryptoKit
import Foundation

/// Key schedule, handshake proofs and frame sealing, byte for byte as in the
/// spec ("Transport and encryption") and `cli/src/mobile/crypto.rs`.
public enum MobileCrypto {
    public static let keySalt = Data("chrome-use mobile v1".utf8)
    static let phoneProofLabel = Data("chrome-use mobile v1 phone proof".utf8)
    static let gatewayProofLabel = Data("chrome-use mobile v1 gateway proof".utf8)
    public static let secretLength = 32
    public static let nonceLength = 32

    /// HKDF-SHA256 (RFC 5869), one 32-byte block. An empty salt is spelled
    /// out as 32 zero bytes, as the gateway does.
    public static func hkdf(ikm: Data, salt: Data, info: Data) -> Data {
        let salt = salt.isEmpty ? Data(repeating: 0, count: 32) : salt
        let key = HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: ikm),
            salt: salt,
            info: info,
            outputByteCount: 32
        )
        return key.withUnsafeBytes { Data($0) }
    }

    public static func hmac(key: Data, _ parts: Data...) -> Data {
        var mac = HMAC<SHA256>(key: SymmetricKey(data: key))
        for part in parts { mac.update(data: part) }
        return Data(mac.finalize())
    }

    public static func random(_ count: Int) -> Data {
        var bytes = [UInt8](repeating: 0, count: count)
        let status = SecRandomCopyBytes(kSecRandomDefault, count, &bytes)
        precondition(status == errSecSuccess, "System randomness is unavailable")
        return Data(bytes)
    }
}

/// Long-term keys of one device. `ikm` is the 32-byte pair secret on the
/// pairing connection, then `pair_secret ‖ phone_secret` (64 bytes).
public struct DeviceKeys: Equatable, Sendable {
    public let auth: Data
    public let frame: Data
    public let push: Data

    public init(ikm: Data) {
        let frame = MobileCrypto.hkdf(ikm: ikm, salt: MobileCrypto.keySalt, info: Data("frame".utf8))
        self.auth = MobileCrypto.hkdf(ikm: ikm, salt: MobileCrypto.keySalt, info: Data("auth".utf8))
        self.frame = frame
        self.push = MobileCrypto.hkdf(ikm: frame, salt: Data(), info: Data("push".utf8))
    }

    /// Per-connection keys: gateway→phone (`c2p`) and phone→gateway (`p2c`).
    public func session(gatewayNonce: Data, phoneNonce: Data) -> (c2p: Data, p2c: Data) {
        let salt = gatewayNonce + phoneNonce
        return (
            MobileCrypto.hkdf(ikm: frame, salt: salt, info: Data("c2p".utf8)),
            MobileCrypto.hkdf(ikm: frame, salt: salt, info: Data("p2c".utf8))
        )
    }

    public func phoneProof(gatewayNonce: Data, phoneNonce: Data, deviceId: String) -> Data {
        MobileCrypto.hmac(key: auth, MobileCrypto.phoneProofLabel, gatewayNonce, phoneNonce, Data(deviceId.utf8))
    }

    public func gatewayProof(gatewayNonce: Data, phoneNonce: Data, deviceId: String) -> Data {
        MobileCrypto.hmac(key: auth, MobileCrypto.gatewayProofLabel, gatewayNonce, phoneNonce, Data(deviceId.utf8))
    }

    /// Constant-time check of the gateway's `ready` proof.
    public func verifyGatewayProof(_ proof: Data, gatewayNonce: Data, phoneNonce: Data, deviceId: String) -> Bool {
        let label = MobileCrypto.gatewayProofLabel + gatewayNonce + phoneNonce + Data(deviceId.utf8)
        return HMAC<SHA256>.isValidAuthenticationCode(proof, authenticating: label, using: SymmetricKey(data: auth))
    }
}

public enum FrameError: Error, Equatable {
    case tooShort
    case outOfSequence(expected: UInt64, got: UInt64)
    case authentication
    case exhausted
}

/// One direction of an encrypted connection: each frame is
/// `seq (8, BE) ‖ ciphertext ‖ tag (16)`, GCM nonce `0000 ‖ seq`, the sequence
/// bytes as associated data. Sequence numbers start at 0 and must be exactly
/// the next one.
public struct FrameChannel: Sendable {
    let key: SymmetricKey
    public private(set) var sequence: UInt64 = 0

    public init(key: Data) {
        self.key = SymmetricKey(data: key)
    }

    static func nonce(_ sequence: UInt64) -> AES.GCM.Nonce {
        var bytes = Data(repeating: 0, count: 4)
        bytes.append(sequence.bigEndianData)
        return try! AES.GCM.Nonce(data: bytes)
    }

    public mutating func seal(_ plaintext: Data) throws -> Data {
        guard sequence < UInt64.max else { throw FrameError.exhausted }
        let header = sequence.bigEndianData
        let box = try AES.GCM.seal(plaintext, using: key, nonce: Self.nonce(sequence), authenticating: header)
        sequence += 1
        return header + box.ciphertext + box.tag
    }

    public mutating func open(_ frame: Data) throws -> Data {
        let frame = Data(frame)
        guard frame.count >= 8 + 16 else { throw FrameError.tooShort }
        let header = frame.prefix(8)
        let got = UInt64(bigEndian: header)
        guard got == sequence else { throw FrameError.outOfSequence(expected: sequence, got: got) }
        let body = frame.dropFirst(8)
        let box: AES.GCM.SealedBox
        do {
            box = try AES.GCM.SealedBox(
                nonce: Self.nonce(got),
                ciphertext: body.dropLast(16),
                tag: body.suffix(16)
            )
        } catch { throw FrameError.authentication }
        guard let plain = try? AES.GCM.open(box, using: key, authenticating: header) else {
            throw FrameError.authentication
        }
        guard sequence < UInt64.max else { throw FrameError.exhausted }
        sequence += 1
        return plain
    }
}

/// End-to-end encrypted push payloads: standard base64 of
/// `kid (4) ‖ nonce (12) ‖ ciphertext ‖ tag`, no associated data, under
/// `push_key`. Payloads from gateways before the key id carry no `kid`.
public enum PushCrypto {
    /// Non-secret id of a push key: the first 4 bytes of
    /// HMAC-SHA256(push_key, "chrome-use mobile v1 kid").
    public static func kid(pushKey: Data) -> Data {
        MobileCrypto.hmac(key: pushKey, Data("chrome-use mobile v1 kid".utf8)).prefix(4)
    }

    /// Open with this key; the `kid` prefix is used when it matches the key,
    /// otherwise the payload is tried as the older unprefixed form.
    public static func open(payload: String, pushKey: Data) -> Data? {
        guard let raw = Data(base64Encoded: payload) else { return nil }
        if raw.count > 4 + 12 + 16, raw.prefix(4) == kid(pushKey: pushKey),
           let plain = openSealed(raw.dropFirst(4), pushKey: pushKey) {
            return plain
        }
        return openSealed(raw, pushKey: pushKey)
    }

    /// Whether the payload names this key (cheap check before decrypting).
    public static func names(payload: String, pushKey: Data) -> Bool {
        guard let raw = Data(base64Encoded: payload), raw.count > 4 else { return false }
        return raw.prefix(4) == kid(pushKey: pushKey)
    }

    static func openSealed(_ raw: Data, pushKey: Data) -> Data? {
        let raw = Data(raw)
        guard raw.count > 12 + 16,
              let box = try? AES.GCM.SealedBox(combined: raw),
              let plain = try? AES.GCM.open(box, using: SymmetricKey(data: pushKey))
        else { return nil }
        return plain
    }

    public static func seal(_ plaintext: Data, pushKey: Data, nonce: Data) throws -> String {
        let box = try AES.GCM.seal(plaintext, using: SymmetricKey(data: pushKey), nonce: AES.GCM.Nonce(data: nonce))
        return box.combined!.base64EncodedString()
    }
}

extension UInt64 {
    var bigEndianData: Data {
        withUnsafeBytes(of: self.bigEndian) { Data($0) }
    }

    init(bigEndian bytes: Data) {
        self = bytes.reduce(0) { ($0 << 8) | UInt64($1) }
    }
}

public enum Base64URL {
    public static func encode(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    /// Unpadded base64url; padded input and the standard alphabet are refused,
    /// as the gateway refuses them.
    public static func decode(_ text: String) -> Data? {
        guard !text.contains("="), !text.contains("+"), !text.contains("/") else { return nil }
        var standard = text
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        let remainder = standard.count % 4
        if remainder == 1 { return nil }
        if remainder > 0 { standard += String(repeating: "=", count: 4 - remainder) }
        return Data(base64Encoded: standard)
    }
}

extension Data {
    public init?(hex: String) {
        guard hex.count.isMultiple(of: 2) else { return nil }
        var bytes = [UInt8]()
        var index = hex.startIndex
        while index < hex.endIndex {
            let next = hex.index(index, offsetBy: 2)
            guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
            bytes.append(byte)
            index = next
        }
        self.init(bytes)
    }

    public var hex: String {
        map { String(format: "%02x", $0) }.joined()
    }
}
