import XCTest
@testable import ChromeUseKit

/// Every value of the spec's "Appendix: test vectors"
/// (docs/superpowers/specs/2026-10-19-mobile-remote-design.md).
final class VectorTests: XCTestCase {
    let pairSecret = Data(0x00...0x1f)
    let phoneSecret = Data(0x80...0x9f)
    let gatewayNonce = Data(0x20...0x3f)
    let phoneNonce = Data(0x40...0x5f)
    let deviceId = "0f8fad5b-d9cb-469f-a165-70867728950e"

    func hex(_ text: String) -> Data { Data(hex: text)! }

    func testPhoneSecretBase64URL() {
        XCTAssertEqual(Base64URL.encode(phoneSecret), "gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8")
        XCTAssertEqual(Base64URL.decode("gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8"), phoneSecret)
    }

    func testPairingKeys() {
        let keys = DeviceKeys(ikm: pairSecret)
        XCTAssertEqual(keys.auth.hex, "a8344e0caf1c7d238af084ccdbeb9d4c2d8ac1abeea4314ca22d1d3aaa47e00e")
        XCTAssertEqual(keys.frame.hex, "1ab8cb618f09a935128d1aca5e0aa708610fd20cb23a3b473e2446052036400d")
        let session = keys.session(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce)
        XCTAssertEqual(session.c2p.hex, "5bc1cde1dbe85a0bafb5c37868e4b8798a5d290b548b2c8eea7831449f33116a")
        XCTAssertEqual(session.p2c.hex, "5f2d6790a8083d40200190969fa564fae1390dacab76bd4318c41185f10a6845")
        let proof = keys.phoneProof(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce, deviceId: deviceId)
        XCTAssertEqual(proof.hex, "8dc9b0be57af6d6329053ba1807835eb333d617132c1f1c00d494fa21b99ec20")
        XCTAssertEqual(Base64URL.encode(proof), "jcmwvlevbWMpBTuhgHg16zM9YXEywfHADUlPohuZ7CA")
        let gateway = keys.gatewayProof(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce, deviceId: deviceId)
        XCTAssertEqual(gateway.hex, "7fcae67185c35186b70f2434a8ba5d593d40b2ea074558601cdd76b1e43f34e6")
        XCTAssertTrue(keys.verifyGatewayProof(gateway, gatewayNonce: gatewayNonce, phoneNonce: phoneNonce, deviceId: deviceId))
        // The phone's own proof must not pass as the gateway's (no reflection).
        XCTAssertFalse(keys.verifyGatewayProof(proof, gatewayNonce: gatewayNonce, phoneNonce: phoneNonce, deviceId: deviceId))
    }

    func testPairCompleteFrame() throws {
        let plaintext = GatewayConnection.pairCompleteFrame(phoneSecret: phoneSecret)
        XCTAssertEqual(
            String(decoding: plaintext, as: UTF8.self),
            #"{"id":0,"op":"pairComplete","params":{"phoneSecret":"gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8"}}"#
        )
        let p2c = DeviceKeys(ikm: pairSecret).session(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce).p2c
        var channel = FrameChannel(key: p2c)
        let frame = try channel.seal(plaintext)
        let expected = hex("0000000000000000eecbbe0b87d98d8799928139b1306a0e8ad641630d702f3912455ee905df518bc47e2a62a8fbf37d44334e959fba8654b2a8ec58ee735b6d9d77cf5a8754981d6b520b9cec3c24c249fcdda6072bd7903904448ba4062b027abc2248c9370df66d6cc1bca2459aebcb0187444d0f2ed86431c5")
        XCTAssertEqual(frame.hex, expected.hex)
        var gatewaySide = FrameChannel(key: p2c)
        XCTAssertEqual(try gatewaySide.open(expected), plaintext)
    }

    func testPairingAcknowledgement() throws {
        let c2p = DeviceKeys(ikm: pairSecret).session(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce).c2p
        let frame = hex("00000000000000002887dc6b0a83e1b96fccac5816546874f376ca967dc9dd031675a37003e6d635896c1495a11d67ef35831329b779c306a8")
        var receiver = FrameChannel(key: c2p)
        let plain = try receiver.open(frame)
        XCTAssertEqual(String(decoding: plain, as: UTF8.self), #"{"id":0,"result":{"paired":true}}"#)
        var sender = FrameChannel(key: c2p)
        XCTAssertEqual(try sender.seal(plain), frame)
    }

    func testPairedKeys() {
        let keys = DeviceKeys(ikm: pairSecret + phoneSecret)
        XCTAssertEqual(keys.auth.hex, "1ff8540b97d115a12bf80351302fc1b97847bf21ad52bef5b46a2e305c1c5ddc")
        XCTAssertEqual(keys.frame.hex, "624f1577d77bcf36468bd37c6acd886a73fce2796128b9130416ec88849f46b3")
        XCTAssertEqual(keys.push.hex, "62ea56e406bb35ebf28f9c403bd6bfd0431aef952cd6d13d6ac50de785e0b621")
        let session = keys.session(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce)
        XCTAssertEqual(session.c2p.hex, "691a14378649adfe6b24fed30a8e12a6465b064ee71bc7f5eba952c04fd2b5b7")
        XCTAssertEqual(session.p2c.hex, "9ae2fd1ae48966a3576dc7b61f2d8b6f59e9913596062e1940aa432dbc46d024")
        XCTAssertEqual(
            keys.phoneProof(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce, deviceId: deviceId).hex,
            "a2766a2b56dfecf634d78a5ff550d23f073cd6bc6a1b66538b0f27461f9c23c3"
        )
        XCTAssertEqual(
            keys.gatewayProof(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce, deviceId: deviceId).hex,
            "3f56ef6947151350afa20a71ef7dfe4af26240a5e3d4814c6292005051e5b55b"
        )
    }

    func testFirstPairedFrame() throws {
        let p2c = DeviceKeys(ikm: pairSecret + phoneSecret).session(gatewayNonce: gatewayNonce, phoneNonce: phoneNonce).p2c
        // Built the way GatewayConnection.request builds every request.
        let request: JSONValue = ["id": 1, "op": "conversationList", "params": [:]]
        XCTAssertEqual(request.encodedString(), #"{"id":1,"op":"conversationList","params":{}}"#)
        var channel = FrameChannel(key: p2c)
        let frame = try channel.seal(request.encoded())
        let expected = "00000000000000003d3fea079d6d776461ebe1024f75712d9eb0b8324463906d3735ad868274cd09d2ed59f4fc423448e2dccb847c9f6b202838c5922a0c6c496cdb5726"
        XCTAssertEqual(frame.hex, expected)
        var gatewaySide = FrameChannel(key: p2c)
        XCTAssertEqual(try gatewaySide.open(hex(expected)), request.encoded())
    }

    /// Spec appendix: push under the paired push key, nonce 60…6b.
    static let pushPayload = "D9OaRmBhYmNkZWZnaGlqa5LWr9sMfSfsQKyuDrzA3lU/VPPJ6gqLhPXaVYvov7CkcK80I1Lyu2LV3h1400pohwLJxAkRdNJDpBOB6YFXBj+KqnEkTNyedZ9NlyimW4JZCLDWHVkisCr6vRg+bP0LTJ9IPNZ/vuEAUEvASnRUoP0fA2z1ChvjIk0tBepBrmiuVxuePSZeJEuygSCkNnBbEVQAqk2LryQAdNZH2Fn72U5ucwevM2p9auOluknJ1H1AWrc="
    static let pushPlaintext = #"{"v":1,"conversationId":"7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e","kind":"needsInput","code":"needsInput","title":"Weekly report","body":"Waiting for your approval"}"#

    func testPushKeyIdAndPayload() throws {
        let pushKey = DeviceKeys(ikm: pairSecret + phoneSecret).push
        XCTAssertEqual(PushCrypto.kid(pushKey: pushKey).hex, "0fd39a46")
        XCTAssertTrue(PushCrypto.names(payload: Self.pushPayload, pushKey: pushKey))
        let plain = try XCTUnwrap(PushCrypto.open(payload: Self.pushPayload, pushKey: pushKey))
        XCTAssertEqual(String(decoding: plain, as: UTF8.self), Self.pushPlaintext)
        XCTAssertEqual(try PushCrypto.seal(Data(Self.pushPlaintext.utf8), pushKey: pushKey, nonce: Data(0x60...0x6b)), Self.pushPayload)
        // Without the kid (or with another key's), the payload is not ours.
        let raw = try XCTUnwrap(Data(base64Encoded: Self.pushPayload))
        XCTAssertNil(PushCrypto.open(payload: raw.dropFirst(4).base64EncodedString(), pushKey: pushKey))
        XCTAssertNil(PushCrypto.open(payload: Self.pushPayload, pushKey: Data(repeating: 3, count: 32)))
    }

    func testPushNoticePicksItsComputerByKid() throws {
        let other = GatewayCredential(url: URL(string: "ws://a")!, deviceId: UUID().uuidString, secret: Data(repeating: 7, count: 64), gatewayId: "x", name: "other", pairedAt: Date())
        let mine = GatewayCredential(url: URL(string: "ws://b")!, deviceId: deviceId, secret: pairSecret + phoneSecret, gatewayId: "y", name: "mine", pairedAt: Date())
        let notice = try XCTUnwrap(PushNotice.open(payload: Self.pushPayload, keys: [other, mine].map(PushKeyEntry.init)))
        XCTAssertEqual(notice.deviceId, deviceId)
        XCTAssertEqual(notice.code, .needsInput)
        XCTAssertEqual(notice.rawKind, "needsInput")
        XCTAssertEqual(notice.conversationId, "7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e")
        XCTAssertEqual(notice.title, "Weekly report")
        XCTAssertFalse(notice.showsBody, "needsInput is worded by its code, not the English body")
        XCTAssertEqual(notice.userInfo["code"], "needsInput")
        XCTAssertNil(PushNotice.open(payload: Self.pushPayload, credentials: [other]), "no matching kid: dropped")
        XCTAssertNil(PushNotice.open(payload: "not base64", credentials: [mine]))
    }

    func testCodesDecideWhatIsShown() throws {
        let key = DeviceKeys(ikm: pairSecret + phoneSecret).push
        let mine = GatewayCredential(url: URL(string: "ws://b")!, deviceId: deviceId, secret: pairSecret + phoneSecret, gatewayId: "y", name: "mine", pairedAt: Date())
        func open(_ code: String, _ body: String) throws -> PushNotice {
            let plain = #"{"v":1,"conversationId":"","kind":"turnEnded","code":"\#(code)","title":"t","body":"\#(body)"}"#
            let payload = try PushCrypto.seal(Data(plain.utf8), pushKey: key, nonce: Data(repeating: 1, count: 12))
            return try XCTUnwrap(PushNotice.open(payload: payload, credentials: [mine]))
        }
        XCTAssertEqual(try open("finished", "Your inbox has 3 unread").code, .finished)
        XCTAssertTrue(try open("finished", "Your inbox has 3 unread").showsBody)
        XCTAssertEqual(try open("test", "Test notification (1)").code, .test)
        XCTAssertFalse(try open("test", "Test notification (1)").showsBody)
        XCTAssertNil(try open("somethingNew", "x").code, "unknown code: generic text")
        XCTAssertFalse(try open("somethingNew", "x").showsBody)
    }
}
