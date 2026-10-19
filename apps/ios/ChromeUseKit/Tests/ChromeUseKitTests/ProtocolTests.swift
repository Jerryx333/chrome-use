import XCTest
@testable import ChromeUseKit

final class FrameChannelTests: XCTestCase {
    let key = Data(repeating: 0x42, count: 32)

    func testRoundTripAndStrictSequence() throws {
        var sender = FrameChannel(key: key)
        var receiver = FrameChannel(key: key)
        let first = try sender.seal(Data("one".utf8))
        let second = try sender.seal(Data("two".utf8))
        XCTAssertEqual(first.prefix(8), Data(repeating: 0, count: 8))
        XCTAssertEqual(second.prefix(8), Data([0, 0, 0, 0, 0, 0, 0, 1]))
        // Reordered: the second frame before the first is refused.
        var reordered = receiver
        XCTAssertThrowsError(try reordered.open(second)) {
            XCTAssertEqual($0 as? FrameError, .outOfSequence(expected: 0, got: 1))
        }
        XCTAssertEqual(try receiver.open(first), Data("one".utf8))
        // Replayed: the same frame twice is refused.
        XCTAssertThrowsError(try receiver.open(first)) {
            XCTAssertEqual($0 as? FrameError, .outOfSequence(expected: 1, got: 0))
        }
        XCTAssertEqual(try receiver.open(second), Data("two".utf8))
        XCTAssertEqual(receiver.sequence, 2)
    }

    func testTamperedFramesFailAuthentication() throws {
        var sender = FrameChannel(key: key)
        let frame = try sender.seal(Data("hello".utf8))
        for index in [8, frame.count - 1] {
            var bad = frame
            bad[index] ^= 0x01
            var receiver = FrameChannel(key: key)
            XCTAssertThrowsError(try receiver.open(bad)) { XCTAssertEqual($0 as? FrameError, .authentication) }
            XCTAssertEqual(receiver.sequence, 0, "a rejected frame does not advance the sequence")
        }
        // The sequence header is authenticated too: relabelling a frame fails.
        var relabelled = frame
        relabelled[7] = 1
        var receiver = FrameChannel(key: key)
        _ = try receiver.open(try FrameChannel(key: key).sealCopy(Data("x".utf8)))
        XCTAssertThrowsError(try receiver.open(relabelled)) { XCTAssertEqual($0 as? FrameError, .authentication) }
        var short = FrameChannel(key: key)
        XCTAssertThrowsError(try short.open(Data(repeating: 0, count: 23))) { XCTAssertEqual($0 as? FrameError, .tooShort) }
    }

    func testWrongDirectionKeyFails() throws {
        let keys = DeviceKeys(ikm: Data(repeating: 1, count: 64)).session(gatewayNonce: Data(repeating: 2, count: 32), phoneNonce: Data(repeating: 3, count: 32))
        var sender = FrameChannel(key: keys.p2c)
        var receiver = FrameChannel(key: keys.c2p)
        XCTAssertThrowsError(try receiver.open(try sender.seal(Data("x".utf8))))
    }
}

extension FrameChannel {
    func sealCopy(_ data: Data) throws -> Data {
        var copy = self
        return try copy.seal(data)
    }
}

final class CloseCodeTests: XCTestCase {
    func testMapping() {
        XCTAssertEqual(CloseReason(closeCode: 4400), .malformed)
        XCTAssertEqual(CloseReason(closeCode: 4401), .unauthorized)
        XCTAssertEqual(CloseReason(closeCode: 4403), .revoked)
        XCTAssertEqual(CloseReason(closeCode: 4408), .handshakeTimeout)
        XCTAssertEqual(CloseReason(closeCode: 4409), .wrongProfile)
        XCTAssertEqual(CloseReason(closeCode: 4429), .backoff)
        XCTAssertEqual(CloseReason(closeCode: 1000), .closed(code: 1000))
        XCTAssertEqual(CloseReason(closeCode: nil), .closed(code: nil))
    }

    func testRetryPolicy() {
        for permanent in [CloseReason.unauthorized, .revoked, .wrongProfile, .gatewayNotAuthentic] {
            XCTAssertTrue(permanent.isPermanent)
            XCTAssertNil(permanent.retryDelay(attempt: 0))
        }
        XCTAssertTrue(CloseReason.revoked.offersRepair)
        XCTAssertTrue(CloseReason.unauthorized.offersRepair)
        XCTAssertFalse(CloseReason.wrongProfile.offersRepair)
        XCTAssertEqual(CloseReason.malformed.retryDelay(attempt: 0), 1)
        XCTAssertEqual(CloseReason.closed(code: nil).retryDelay(attempt: 3), 8)
        XCTAssertEqual(CloseReason.closed(code: nil).retryDelay(attempt: 50), 30)
        XCTAssertEqual(CloseReason.handshakeTimeout.retryDelay(attempt: 1), 2)
        // Backed off by the gateway: wait longer, up to its 300 s ceiling.
        XCTAssertEqual(CloseReason.backoff.retryDelay(attempt: 0), 5)
        XCTAssertEqual(CloseReason.backoff.retryDelay(attempt: 10), 300)
    }
}

final class PairLinkTests: XCTestCase {
    let secret = Base64URL.encode(Data(0x00...0x1f))
    let fp = Base64URL.encode(Data(repeating: 9, count: 16))
    let id = "0f8fad5b-d9cb-469f-a165-70867728950e"

    func link(_ url: String = "wss%3A%2F%2Fmac.tail1234.ts.net%2Fphone", v: String = "1", k: String? = nil, fp: String? = nil, id: String? = nil) -> String {
        "chromeuse://pair?v=\(v)&url=\(url)&id=\(id ?? self.id)&k=\(k ?? secret)&fp=\(fp ?? self.fp)"
    }

    func testParses() throws {
        let parsed = try PairLink(parsing: link())
        XCTAssertEqual(parsed.url.absoluteString, "wss://mac.tail1234.ts.net/phone")
        XCTAssertEqual(parsed.deviceId, id)
        XCTAssertEqual(parsed.pairSecret, Data(0x00...0x1f))
        XCTAssertEqual(parsed.gatewayId, fp)
        XCTAssertEqual(parsed.suggestedName, "mac.tail1234.ts.net")
        XCTAssertNoThrow(try PairLink(parsing: "  " + link("ws%3A%2F%2F100.64.1.2%3A17345") + "\n"))
    }

    func testRejects() {
        XCTAssertThrowsError(try PairLink(parsing: "https://example.com")) { XCTAssertEqual($0 as? PairLink.ParseError, .notAPairLink) }
        XCTAssertThrowsError(try PairLink(parsing: link(v: "2"))) { XCTAssertEqual($0 as? PairLink.ParseError, .unsupportedVersion) }
        XCTAssertThrowsError(try PairLink(parsing: link("https%3A%2F%2Fexample.com"))) { XCTAssertEqual($0 as? PairLink.ParseError, .badURL) }
        XCTAssertThrowsError(try PairLink(parsing: link(id: "not-a-uuid"))) { XCTAssertEqual($0 as? PairLink.ParseError, .badDeviceId) }
        XCTAssertThrowsError(try PairLink(parsing: link(k: Base64URL.encode(Data(count: 16))))) { XCTAssertEqual($0 as? PairLink.ParseError, .badSecret) }
        XCTAssertThrowsError(try PairLink(parsing: link(k: secret + "="))) { XCTAssertEqual($0 as? PairLink.ParseError, .badSecret) }
        XCTAssertThrowsError(try PairLink(parsing: link(fp: "short"))) { XCTAssertEqual($0 as? PairLink.ParseError, .badGatewayId) }
    }
}

final class EntitlementTests: XCTestCase {
    let start = Date(timeIntervalSince1970: 1_800_000_000)

    func at(_ offset: TimeInterval) -> FixedClock { FixedClock(start.addingTimeInterval(offset)) }

    func testUnlockedAlwaysWrites() {
        let state = Entitlement.evaluate(unlocked: true, trialStart: start, clock: at(365 * 86_400))
        XCTAssertEqual(state, .unlocked)
        XCTAssertTrue(state.canWrite)
        XCTAssertEqual(Entitlement.evaluate(unlocked: true, trialStart: nil, clock: at(0)), .unlocked)
    }

    func testTrialNotStartedCannotWrite() {
        let state = Entitlement.evaluate(unlocked: false, trialStart: nil, clock: at(0))
        XCTAssertEqual(state, .trialNotStarted)
        XCTAssertFalse(state.canWrite)
    }

    func testTrialWindow() {
        let end = start.addingTimeInterval(14 * 86_400)
        let first = Entitlement.evaluate(unlocked: false, trialStart: start, clock: at(0))
        XCTAssertEqual(first, .trial(endsAt: end))
        XCTAssertTrue(first.canWrite)
        XCTAssertEqual(first.daysLeft(clock: at(0)), 14)
        XCTAssertEqual(first.daysLeft(clock: at(13.5 * 86_400)), 1)
        let lastSecond = Entitlement.evaluate(unlocked: false, trialStart: start, clock: at(14 * 86_400 - 1))
        XCTAssertTrue(lastSecond.canWrite)
    }

    func testExpired() {
        let end = start.addingTimeInterval(14 * 86_400)
        let state = Entitlement.evaluate(unlocked: false, trialStart: start, clock: at(14 * 86_400))
        XCTAssertEqual(state, .expired(endedAt: end))
        XCTAssertFalse(state.canWrite)
        XCTAssertEqual(state.daysLeft(clock: at(20 * 86_400)), 0)
    }
}

final class RelayRegistrationTests: XCTestCase {
    func testRefresh() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let registration = RelayRegistration(relayToken: "t", expiresAt: now.addingTimeInterval(30 * 86_400), deviceToken: "abc")
        XCTAssertFalse(registration.needsRefresh(deviceToken: "abc", clock: FixedClock(now)))
        XCTAssertTrue(registration.needsRefresh(deviceToken: "def", clock: FixedClock(now)))
        XCTAssertTrue(registration.needsRefresh(deviceToken: "abc", clock: FixedClock(now.addingTimeInterval(24 * 86_400))))
    }

    func testExpiryFormats() {
        XCTAssertEqual(PushRegistrar.date(.int(1_800_000_000)), Date(timeIntervalSince1970: 1_800_000_000))
        XCTAssertEqual(PushRegistrar.date(.int(1_800_000_000_000)), Date(timeIntervalSince1970: 1_800_000_000))
        XCTAssertEqual(PushRegistrar.date("1970-01-01T00:00:10Z"), Date(timeIntervalSince1970: 10))
        XCTAssertEqual(PushRegistrar.date("1970-01-01T00:00:10.500Z"), Date(timeIntervalSince1970: 10.5))
        XCTAssertNil(PushRegistrar.date(.null))
    }
}

final class TransportPolicyTests: XCTestCase {
    func testPlaintextOnlyInsideTheTailnetOrLAN() {
        for host in ["127.0.0.1", "localhost", "10.0.0.5", "172.16.0.1", "172.31.255.1", "192.168.1.20", "169.254.3.4",
                     "100.64.0.1", "100.101.102.103", "100.127.255.254", "mac.tail1234.ts.net", "studio.local", "macbook",
                     "::1", "fd7a:115c:a1e0::1", "fe80::1"] {
            XCTAssertTrue(TransportPolicy.allowsPlaintext(host: host), host)
        }
        for host in ["example.com", "8.8.8.8", "172.32.0.1", "100.128.0.1", "100.63.255.255", "192.169.0.1",
                     "mac.example.ts.net.evil.com", "2001:db8::1", "tunnel.trycloudflare.com"] {
            XCTAssertFalse(TransportPolicy.allowsPlaintext(host: host), host)
        }
    }

    func testPairLinkRefusesPlaintextToAPublicHost() {
        let secret = Base64URL.encode(Data(0x00...0x1f))
        let fp = Base64URL.encode(Data(repeating: 9, count: 16))
        let link = { (url: String) in "chromeuse://pair?v=1&url=\(url)&id=0f8fad5b-d9cb-469f-a165-70867728950e&k=\(secret)&fp=\(fp)" }
        XCTAssertThrowsError(try PairLink(parsing: link("ws%3A%2F%2Fgateway.example.com"))) {
            XCTAssertEqual($0 as? PairLink.ParseError, .plaintextToPublicHost)
        }
        XCTAssertNoThrow(try PairLink(parsing: link("wss%3A%2F%2Fgateway.example.com")))
        XCTAssertNoThrow(try PairLink(parsing: link("ws%3A%2F%2F100.88.1.2%3A17345")))
        XCTAssertNoThrow(try PairLink(parsing: link("ws%3A%2F%2Fmac.tail1234.ts.net%3A17345")))
    }
}

final class GatedActionTests: XCTestCase {
    func testStopAndRefusalsAreFree() {
        XCTAssertFalse(GatedAction.stop.needsUnlock)
        for kind in [DecisionOption.Kind.deny, .decline, .cancel, .agentOption(name: "Reject", allows: false)] {
            XCTAssertFalse(GatedAction.answer(DecisionOption(kind: kind, decision: "x")).needsUnlock, "\(kind)")
        }
        for kind in [DecisionOption.Kind.allowOnce, .allowForSession, .allowTurn, .agentOption(name: "Allow", allows: true)] {
            XCTAssertTrue(GatedAction.answer(DecisionOption(kind: kind, decision: "x")).needsUnlock, "\(kind)")
        }
        for action in [GatedAction.send, .create, .runSchedule, .toggleSchedule, .answerQuestions] {
            XCTAssertTrue(action.needsUnlock, "\(action)")
        }
    }
}

final class CreateRulesTests: XCTestCase {
    func testPiIsBrowserOnlyAndOthersUpToWriteFiles() {
        XCTAssertEqual(ToolAccess.phoneCreateChoices(agentId: "pi"), [.browserOnly])
        XCTAssertEqual(ToolAccess.phoneCreateChoices(agentId: "claude"), [.browserOnly, .readFiles, .writeFiles])
    }

    func testModelNamesThatReadAsFlagsAreRefused() {
        XCTAssertTrue(AgentModel.isValidName("claude-opus-5-5"))
        XCTAssertTrue(AgentModel.isValidName(""))
        XCTAssertFalse(AgentModel.isValidName("-x"))
        XCTAssertFalse(AgentModel.isValidName("  --dangerously-skip-permissions"))
    }
}

final class LocalNetworkTests: XCTestCase {
    func testWhichHostsNeedThePermission() {
        for host in ["192.168.0.190", "10.1.2.3", "172.20.0.4", "169.254.1.1", "studio.local", "macbook", "fe80::1", "fd00::5"] {
            XCTAssertTrue(LocalNetwork.needsPermission(host: host), host)
        }
        for host in ["100.88.1.2", "mac.tail1234.ts.net", "fd7a:115c:a1e0::1", "127.0.0.1", "localhost", "example.com", "8.8.8.8"] {
            XCTAssertFalse(LocalNetwork.needsPermission(host: host), host)
        }
    }

    func testRetriesOnlyPlainNetworkFailuresOnTheLAN() {
        let network = GatewayError.closed(.closed(code: nil))
        XCTAssertEqual(LocalNetwork.pairingRetryDelay(after: network, attempt: 1, host: "192.168.0.190"), 5)
        XCTAssertEqual(LocalNetwork.pairingRetryDelay(after: network, attempt: 3, host: "192.168.0.190"), 15)
        XCTAssertNil(LocalNetwork.pairingRetryDelay(after: network, attempt: 4, host: "192.168.0.190"), "about 30 s in all")
        XCTAssertNil(LocalNetwork.pairingRetryDelay(after: network, attempt: 1, host: "100.88.1.2"), "Tailscale needs no prompt")
        XCTAssertNil(LocalNetwork.pairingRetryDelay(after: GatewayError.closed(.unauthorized), attempt: 1, host: "192.168.0.190"))
        XCTAssertNil(LocalNetwork.pairingRetryDelay(after: GatewayError.closed(.handshakeTimeout), attempt: 1, host: "192.168.0.190"))
    }
}

final class ElevatedTests: XCTestCase {
    func testMirrorsTheGatewayRule() {
        XCTAssertTrue(ToolAccess.elevated(agentId: "claude", toolAccess: nil, fullAccess: true))
        XCTAssertTrue(ToolAccess.elevated(agentId: "claude", toolAccess: .runCommands, fullAccess: false))
        XCTAssertTrue(ToolAccess.elevated(agentId: "opencode", toolAccess: nil, fullAccess: false))
        XCTAssertTrue(ToolAccess.elevated(agentId: "pi", toolAccess: .readFiles, fullAccess: false))
        XCTAssertFalse(ToolAccess.elevated(agentId: "pi", toolAccess: nil, fullAccess: false))
        XCTAssertFalse(ToolAccess.elevated(agentId: "claude", toolAccess: .writeFiles, fullAccess: false))
        XCTAssertFalse(ToolAccess.elevated(agentId: "codex", toolAccess: nil, fullAccess: false))
        XCTAssertTrue(ConversationSummary(["id": "x", "agentId": "claude", "fullAccess": true]).elevated)
        XCTAssertTrue(Schedule(["id": "s", "agentId": "opencode"]).elevated)
        XCTAssertFalse(Schedule(["id": "s", "agentId": "claude", "options": ["toolAccess": "browserOnly"]]).elevated)
    }
}

final class OutgoingAndWorkerTests: XCTestCase {
    func testMessageIdIsKeptUntilConfirmed() {
        var outgoing = OutgoingMessage()
        let image = Data(repeating: 1, count: 100)
        let first = outgoing.id(text: "hello", images: [image])
        XCTAssertEqual(outgoing.id(text: "hello", images: [image]), first, "a retry reuses the id")
        let edited = outgoing.id(text: "hello!", images: [image])
        XCTAssertNotEqual(edited, first, "changed text: the service would refuse the old id")
        XCTAssertNotEqual(outgoing.id(text: "hello!", images: []), edited, "changed images too")
        let current = outgoing.id(text: "next")
        outgoing.sent()
        XCTAssertNotEqual(outgoing.id(text: "next"), current, "after confirmation a new message gets a new id")
        XCTAssertNotNil(UUID(uuidString: current))
    }

    func testPushWorkerMustBeHTTPS() {
        XCTAssertEqual(PushRegistrar.workerURL(" https://push.example.com ")?.host, "push.example.com")
        XCTAssertNil(PushRegistrar.workerURL("http://push.example.com"))
        XCTAssertNil(PushRegistrar.workerURL("https://"))
        XCTAssertNil(PushRegistrar.workerURL("ftp://x"))
        XCTAssertNil(PushRegistrar.workerURL("push.example.com"))
    }
}
