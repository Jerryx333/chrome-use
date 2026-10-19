import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
import XCTest
@testable import ChromeUseKit

func event(_ sequence: Int64, _ kind: String, _ data: JSONValue) -> JSONValue {
    ["sequence": .int(sequence), "kind": .string(kind), "data": data, "at": .int(1_800_000_000_000 + sequence)]
}

final class TimelineTests: XCTestCase {
    func testTextDeltasAndFinal() throws {
        var timeline = Timeline()
        try timeline.apply(events: [
            event(1, "user", ["messageId": "m1", "text": "Summarize my inbox"]),
            event(2, "textDelta", ["messageId": "m1", "itemId": "a", "text": "Hel"]),
            event(3, "textDelta", ["messageId": "m1", "itemId": "a", "text": "lo"]),
        ], cursor: 3)
        XCTAssertEqual(timeline.rows.count, 2)
        guard case .assistant(_, let streamed, _) = timeline.rows[1] else { return XCTFail() }
        XCTAssertEqual(streamed, "Hello")
        // A final text replaces the streamed one, part by part.
        try timeline.apply(events: [
            event(4, "textFinal", ["messageId": "m1", "itemId": "a", "part": 0, "text": "Hello, "]),
            event(5, "textFinal", ["messageId": "m1", "itemId": "a", "part": 1, "text": "world"]),
        ], cursor: 5)
        guard case .assistant(_, let final, _) = timeline.rows[1] else { return XCTFail() }
        XCTAssertEqual(final, "Hello, world")
        XCTAssertThrowsError(try timeline.apply(events: [
            event(6, "textFinal", ["messageId": "m1", "itemId": "a", "part": 5, "text": "x"]),
        ], cursor: 6)) { XCTAssertEqual($0 as? TimelineError, .chunksOutOfOrder) }
    }

    func testRepeatedEventsAreSkippedAndGapsRefused() throws {
        var timeline = Timeline()
        let first = event(1, "user", ["messageId": "m1", "text": "hi"])
        try timeline.apply(events: [first], cursor: 1)
        // A resumed subscription may repeat what was already seen.
        try timeline.apply(events: [first], cursor: 1)
        XCTAssertEqual(timeline.rows.count, 1)
        XCTAssertThrowsError(try timeline.apply(events: [event(3, "user", ["messageId": "m3", "text": "x"])], cursor: 3)) {
            XCTAssertEqual($0 as? TimelineError, .historyGap(expected: 2, got: 3))
        }
        XCTAssertThrowsError(try timeline.apply(events: [], cursor: 9)) {
            XCTAssertEqual($0 as? TimelineError, .cursorMismatch(expected: 1, got: 9))
        }
    }

    func testToolsAreCompactAndHiddenItemsStayHidden() throws {
        var timeline = Timeline()
        try timeline.apply(events: [
            event(1, "tool", ["messageId": "m1", "item": ["id": "t1", "tool": "mcp__chrome_use__chrome_use_open", "input": ["url": "https://example.com/inbox"]]]),
            event(2, "tool", ["messageId": "m1", "item": ["id": "t1", "tool": "mcp__chrome_use__chrome_use_open", "input": ["url": "https://example.com/inbox"]], "completed": true]),
            event(3, "tool", ["messageId": "m1", "item": ["id": "r", "type": "reasoning"]]),
            event(4, "tool", ["messageId": "m1", "item": ["id": "c", "type": "commandExecution", "command": "/bin/zsh -lc ls -la", "status": "failed"]]),
        ], cursor: 4)
        XCTAssertEqual(timeline.rows.count, 2)
        guard case .tool(_, let name, let hint, let failed, let completed, let browser) = timeline.rows[0] else { return XCTFail() }
        XCTAssertEqual(name, "browser:open")
        XCTAssertEqual(hint, "https://example.com/inbox")
        XCTAssertFalse(failed)
        XCTAssertTrue(completed)
        XCTAssertTrue(browser)
        guard case .tool(_, let command, let commandHint, let commandFailed, _, _) = timeline.rows[1] else { return XCTFail() }
        XCTAssertEqual(command, "command")
        XCTAssertEqual(commandHint, "ls -la")
        XCTAssertTrue(commandFailed)
    }

    func testRequestsRevisionsAndResolution() throws {
        var timeline = Timeline()
        let request: JSONValue = ["requestId": 7, "method": "can_use_tool", "generation": 1, "requestRevision": 3, "params": ["tool_name": "Bash", "input": ["command": "ls"]]]
        try timeline.apply(events: [event(1, "request", request)], cursor: 1)
        XCTAssertEqual(timeline.openRequests.map(\.key), ["7#3"])
        // A newer revision of the same request id retires the older card.
        var revised = request
        if case .object(var map) = revised { map["requestRevision"] = 4; revised = .object(map) }
        try timeline.apply(events: [event(2, "request", revised)], cursor: 2)
        XCTAssertEqual(timeline.openRequests.map(\.key), ["7#4"])
        try timeline.apply(events: [event(3, "replySent", ["requestId": 7])], cursor: 3)
        XCTAssertEqual(timeline.openRequests.count, 0)
        XCTAssertEqual(timeline.requests.count, 1)
        try timeline.apply(events: [event(4, "requestResolved", ["requestId": 7])], cursor: 4)
        XCTAssertEqual(timeline.requests.count, 0)
        try timeline.apply(events: [event(5, "request", request), event(6, "terminal", ["status": "failed", "error": #"{"error":{"message":"Model overloaded"}}"#])], cursor: 6)
        XCTAssertEqual(timeline.requests.count, 0, "a terminal event clears open requests")
        guard case .note(_, let text, let failed) = timeline.rows.last else { return XCTFail() }
        XCTAssertEqual(text, "Model overloaded")
        XCTAssertTrue(failed)
    }
}

final class ApprovalTests: XCTestCase {
    func request(_ method: String, _ params: JSONValue, bound: Bool = true) -> PendingRequest {
        PendingRequest(key: "k", requestId: 7, method: method, params: params, generation: bound ? 1 : nil, requestRevision: bound ? 3 : nil, sent: false)
    }

    func decisions(_ form: ApprovalForm) -> [JSONValue] {
        guard case .decisions(_, _, let options) = form else { return [] }
        return options.map(\.decision)
    }

    func testDecisionsPerBackendMethod() {
        XCTAssertEqual(decisions(.build(request("can_use_tool", ["tool_name": "Bash", "input": ["command": "ls"]]))), ["allow", "deny"])
        XCTAssertEqual(decisions(.build(request("item/commandExecution/requestApproval", ["command": "ls"]))), ["accept", "acceptForSession", "decline", "cancel"])
        XCTAssertEqual(decisions(.build(request("item/fileChange/requestApproval", [:]))), ["accept", "acceptForSession", "decline", "cancel"])
        XCTAssertEqual(decisions(.build(request("item/permissions/requestApproval", ["permissions": [:]]))), ["accept"])
        XCTAssertEqual(decisions(.build(request("session/request_permission", ["options": [["optionId": "once", "name": "Allow once", "kind": "allow_once"], ["optionId": "no", "name": "Reject", "kind": "reject_once"]]]))),
                       [["optionId": "once"], ["optionId": "no"], "cancel"])
        // An arbitrary elicitation form cannot be accepted with empty content.
        XCTAssertEqual(decisions(.build(request("mcpServer/elicitation/request", ["serverName": "other", "mode": "form"]))), ["decline", "cancel"])
        let safe: JSONValue = ["serverName": "chrome_use", "mode": "form", "requestedSchema": ["type": "object", "properties": [:]], "_meta": ["codex_approval_kind": "mcp_tool_call", "tool_params": ["url": "https://example.com"]]]
        XCTAssertEqual(decisions(.build(request("mcpServer/elicitation/request", safe))), ["accept", "decline", "cancel"])
    }

    func testQuestions() {
        let claude = ApprovalForm.build(request("can_use_tool", ["tool_name": "AskUserQuestion", "input": ["questions": [["question": "Which?", "options": [["label": "A"], ["label": "B"]], "multiSelect": false]]]]))
        guard case .questions(let questions, let deny) = claude else { return XCTFail() }
        XCTAssertEqual(questions.first?.options.map(\.label), ["A", "B"])
        XCTAssertTrue(questions[0].allowsCustom)
        XCTAssertEqual(deny?.decision, "deny")
        let codex = ApprovalForm.build(request("item/tool/requestUserInput", ["questions": [["id": "q", "question": "Pick", "options": [["label": "x"]]], ["id": "free", "question": "Why"]]]))
        guard case .questions(let codexQuestions, let codexDeny) = codex else { return XCTFail() }
        XCTAssertNil(codexDeny)
        XCTAssertFalse(codexQuestions[0].allowsCustom)
        XCTAssertTrue(codexQuestions[1].allowsCustom)
        XCTAssertEqual(
            ApprovalForm.answers([.option(1), .options([0, 2]), .custom("because")]).encodedString(),
            #"{"answersByPosition":[{"option":1},{"options":[0,2]},{"custom":"because"}]}"#
        )
    }

    func testOnlyBoundRepliesAndTruncation() {
        XCTAssertEqual(ApprovalForm.build(request("can_use_tool", ["tool_name": "Bash"], bound: false)), .unsupported(method: "can_use_tool"))
        XCTAssertEqual(ApprovalForm.build(request("can_use_tool", ["truncated": true])), .truncated)
        XCTAssertEqual(ApprovalForm.build(request("something/new", [:])), .unsupported(method: "something/new"))
        let params = ApprovalForm.respondParams(conversationId: "c", request: request("can_use_tool", [:]), decision: "allow")
        XCTAssertEqual(params.encodedString(), #"{"conversationId":"c","decision":"allow","generation":1,"requestId":7,"requestRevision":3}"#)
    }
}

final class ImagePreparerTests: XCTestCase {
    func png(width: Int, height: Int) -> Data {
        let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        // Noise compresses badly, which forces the size loop to work.
        var generator = SystemRandomNumberGenerator()
        for y in stride(from: 0, to: height, by: 4) {
            for x in stride(from: 0, to: width, by: 4) {
                context.setFillColor(CGColor(red: .random(in: 0...1, using: &generator), green: .random(in: 0...1, using: &generator), blue: .random(in: 0...1, using: &generator), alpha: 1))
                context.fill(CGRect(x: x, y: y, width: 4, height: 4))
            }
        }
        let output = NSMutableData()
        let destination = CGImageDestinationCreateWithData(output, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(destination, context.makeImage()!, nil)
        CGImageDestinationFinalize(destination)
        return output as Data
    }

    func testLargeImageFitsTheLimit() throws {
        let source = png(width: 3000, height: 2000)
        XCTAssertGreaterThan(source.count, ImagePreparer.maximumBytes)
        let jpeg = try ImagePreparer.jpeg(from: source)
        XCTAssertLessThanOrEqual(jpeg.count, ImagePreparer.maximumBytes)
        XCTAssertEqual(jpeg.prefix(2), Data([0xFF, 0xD8]))
        let image = CGImageSourceCreateImageAtIndex(CGImageSourceCreateWithData(jpeg as CFData, nil)!, 0, nil)!
        XCTAssertLessThanOrEqual(max(image.width, image.height), 2000)
    }

    func testGarbageIsRefused() {
        XCTAssertThrowsError(try ImagePreparer.jpeg(from: Data("nope".utf8)))
    }
}

@MainActor
final class GatewayClientTests: XCTestCase {
    func testJournalFramesUpdateTimelineAndCache() throws {
        let credential = GatewayCredential(url: URL(string: "ws://127.0.0.1:1")!, deviceId: UUID().uuidString.lowercased(), secret: Data(count: 64), gatewayId: "g", name: "Mac", pairedAt: Date())
        let cache = InMemoryJournalCache()
        let conversation = "7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e"
        let client = GatewayClient(credential: credential, cache: cache)
        client.subscribe(conversation)
        client.handle(["event": "journal", "conversationId": .string(conversation), "conversation": ["id": .string(conversation), "phase": "running", "title": "Weekly"], "events": [event(1, "user", ["messageId": "m", "text": "hi"])], "cursor": 1, "more": false, "queuedMessages": []])
        XCTAssertEqual(client.cursor(of: conversation), 1)
        XCTAssertEqual(client.summaries[conversation]?.phase, .running)
        XCTAssertEqual(cache.load(deviceId: credential.deviceId, conversationId: conversation)?.cursor, 1)
        // A fresh client (cold start) resumes from the cached cursor.
        let again = GatewayClient(credential: credential, cache: cache)
        again.subscribe(conversation)
        XCTAssertEqual(again.cursor(of: conversation), 1)
        XCTAssertEqual(again.timelines[conversation]?.rows.count, 1)
        // A gap resets the conversation rather than showing a broken history.
        again.handle(["event": "journal", "conversationId": .string(conversation), "events": [event(5, "user", ["messageId": "x", "text": "x"])], "cursor": 5])
        XCTAssertEqual(again.cursor(of: conversation), 0)
        XCTAssertNil(cache.load(deviceId: credential.deviceId, conversationId: conversation))
        again.handle(["event": "subscriptionEnded", "conversationId": .string(conversation), "reason": "gone"])
        XCTAssertTrue(again.ended.contains(conversation))
    }
}

@MainActor
final class HelloAndRejectionTests: XCTestCase {
    func credential(custom: Bool) -> GatewayCredential {
        GatewayCredential(url: URL(string: "ws://127.0.0.1:1")!, deviceId: UUID().uuidString.lowercased(), secret: Data(count: 64), gatewayId: "g", name: custom ? "My desk" : "127.0.0.1", pairedAt: Date(), nameIsCustom: custom)
    }

    func testHelloNamesTheComputerUnlessTheUserDid() {
        let client = GatewayClient(credential: credential(custom: false), cache: InMemoryJournalCache())
        var saved: [GatewayCredential] = []
        client.onCredentialChanged = { saved.append($0) }
        client.applyHello(["computerName": "Leo’s MacBook Pro", "gatewayVersion": "1.5.250"])
        XCTAssertEqual(client.credential.name, "Leo’s MacBook Pro")
        XCTAssertEqual(client.credential.gatewayVersion, "1.5.250")
        XCTAssertEqual(saved.count, 1)
        client.applyHello(["computerName": "Leo’s MacBook Pro", "gatewayVersion": "1.5.250"])
        XCTAssertEqual(saved.count, 1, "nothing changed, nothing saved")

        let custom = GatewayClient(credential: credential(custom: true), cache: InMemoryJournalCache())
        custom.applyHello(["computerName": "Leo’s MacBook Pro", "gatewayVersion": "1.5.251"])
        XCTAssertEqual(custom.credential.name, "My desk")
        XCTAssertEqual(custom.credential.gatewayVersion, "1.5.251")
    }

    func testRejectedTokenEventReachesTheApp() {
        let client = GatewayClient(credential: credential(custom: false), cache: InMemoryJournalCache())
        var reasons: [String] = []
        client.onPushTokenRejected = { reasons.append($0) }
        client.handle(["event": "pushTokenRejected", "reason": "expired"])
        XCTAssertEqual(reasons, ["expired"])
    }

    func testOlderStoredCredentialsStillDecode() throws {
        let old = #"{"url":"ws:\/\/127.0.0.1:1","deviceId":"0f8fad5b-d9cb-469f-a165-70867728950e","secret":"AAAA","gatewayId":"g","name":"Mac","pairedAt":0}"#
        let decoded = try JSONDecoder().decode(GatewayCredential.self, from: Data(old.utf8))
        XCTAssertFalse(decoded.nameIsCustom)
        XCTAssertNil(decoded.gatewayVersion)
        XCTAssertEqual(decoded.name, "Mac")
    }

    func testPairLinkCarriesTheComputerName() throws {
        let secret = Base64URL.encode(Data(0x00...0x1f))
        let fp = Base64URL.encode(Data(repeating: 9, count: 16))
        let base = "chromeuse://pair?v=1&url=ws%3A%2F%2F100.88.1.2%3A17345&id=0f8fad5b-d9cb-469f-a165-70867728950e&k=\(secret)&fp=\(fp)"
        let named = try PairLink(parsing: base + "&n=Leo%E2%80%99s%20Mac%20%E5%B7%A5%E4%BD%9C%E5%AE%A4")
        XCTAssertEqual(named.computerName, "Leo’s Mac 工作室")
        XCTAssertEqual(named.suggestedName, "Leo’s Mac 工作室")
        let unnamed = try PairLink(parsing: base)
        XCTAssertNil(unnamed.computerName)
        XCTAssertEqual(unnamed.suggestedName, "100.88.1.2")
    }
}

final class CredentialSplitTests: XCTestCase {
    func credential(_ gateway: String = "g", device: String = UUID().uuidString.lowercased()) -> GatewayCredential {
        GatewayCredential(url: URL(string: "ws://127.0.0.1:1")!, deviceId: device, secret: Data(repeating: 9, count: 64), gatewayId: gateway, name: "Mac", pairedAt: Date())
    }

    func testMigrationMovesFullCredentialsAndLeavesOnlyPushKeysShared() throws {
        let old = [credential(), credential("h")]
        let legacy = InMemoryCredentialStore(old)
        let credentials = InMemoryCredentialStore()
        let pushKeys = InMemoryPushKeyStore()
        XCTAssertEqual(try CredentialMigration.run(legacy: legacy, into: credentials, pushKeys: pushKeys), 2)
        XCTAssertTrue(try legacy.all().isEmpty)
        XCTAssertEqual(Set(try credentials.all()), Set(old))
        let entries = try pushKeys.all()
        XCTAssertEqual(Set(entries.map(\.deviceId)), Set(old.map(\.deviceId)))
        for entry in entries {
            let source = old.first { $0.deviceId == entry.deviceId }!
            XCTAssertEqual(entry.pushKey, source.keys.push)
            XCTAssertEqual(entry.kid, PushCrypto.kid(pushKey: source.keys.push))
            let encoded = String(decoding: try JSONEncoder().encode(entry), as: UTF8.self)
            XCTAssertFalse(encoded.contains(source.secret.base64EncodedString()), "no pairing secret in the shared entry")
        }
        XCTAssertEqual(try CredentialMigration.run(legacy: legacy, into: credentials, pushKeys: pushKeys), 0, "idempotent")
    }

    func testStorageKeyedByGatewayAndDevice() throws {
        let device = UUID().uuidString.lowercased()
        let store = InMemoryCredentialStore()
        let home = credential("home", device: device)
        let office = credential("office", device: device)
        try store.save(home)
        try store.save(office)
        XCTAssertEqual(try store.all().count, 2, "another gateway cannot overwrite the item")
        try store.delete(storageKey: office.storageKey)
        XCTAssertEqual(try store.all(), [home])
    }

    func testConflictDetection() {
        let saved = credential("home")
        let same = PairLink(url: saved.url, deviceId: saved.deviceId, pairSecret: Data(count: 32), gatewayId: "home")
        XCTAssertFalse(same.conflicts(with: saved))
        XCTAssertTrue(PairLink(url: URL(string: "ws://10.0.0.2:1")!, deviceId: saved.deviceId, pairSecret: Data(count: 32), gatewayId: "home").conflicts(with: saved))
        XCTAssertTrue(PairLink(url: saved.url, deviceId: saved.deviceId, pairSecret: Data(count: 32), gatewayId: "office").conflicts(with: saved))
        XCTAssertFalse(PairLink(url: URL(string: "ws://10.0.0.2:1")!, deviceId: UUID().uuidString, pairSecret: Data(count: 32), gatewayId: "x").conflicts(with: saved))
    }

    @MainActor
    func testHelloRemembersFullAccessAllowed() {
        let client = GatewayClient(credential: credential(), cache: InMemoryJournalCache())
        XCTAssertFalse(client.credential.fullAccessAllowed)
        client.applyHello(["computerName": "Mac", "gatewayVersion": "1", "pushMutes": [], "fullAccessAllowed": true])
        XCTAssertTrue(client.credential.fullAccessAllowed)
        client.applyHello(["computerName": "Mac", "gatewayVersion": "1", "pushMutes": []])
        XCTAssertTrue(client.credential.fullAccessAllowed, "an older gateway without the field changes nothing")
        client.applyHello(["computerName": "Mac", "gatewayVersion": "1", "pushMutes": [], "fullAccessAllowed": false])
        XCTAssertFalse(client.credential.fullAccessAllowed)
    }
}
