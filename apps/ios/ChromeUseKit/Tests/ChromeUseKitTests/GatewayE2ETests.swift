#if os(macOS)
import Foundation
import XCTest
@testable import ChromeUseKit

/// The Swift connection layer against a real `chrome-use mobile serve` built
/// from this branch, a fake panel service and a fake push Worker
/// (Fixtures/fake_panel.py), in a temporary HOME and CHROME_USE_RELAY_DIR.
/// Runs only when CHROME_USE_E2E_BIN names the chrome-use binary.
final class GatewayE2ETests: XCTestCase {
    let conversation = "7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e"

    final class Lines: @unchecked Sendable {
        private var lines: [JSONValue] = []
        private let lock = NSLock()
        func append(_ value: JSONValue) { lock.withLock { lines.append(value) } }
        func first(where match: (JSONValue) -> Bool) -> JSONValue? { lock.withLock { lines.first(where: match) } }
    }

    struct Timeout: Error { let what: String }

    var processes: [Process] = []
    var temp: URL!

    override func tearDown() {
        for process in processes where process.isRunning { process.terminate() }
        processes.removeAll()
        if let temp { try? FileManager.default.removeItem(at: temp) }
    }

    func environment(relay: URL, home: URL) -> [String: String] {
        var env = ProcessInfo.processInfo.environment
        env["CHROME_USE_RELAY_DIR"] = relay.path
        env["HOME"] = home.path
        env["CHROME_USE_NO_UPDATE_CHECK"] = "1"
        return env
    }

    /// Start a process; its stdout lines (JSON) are collected.
    func launch(_ executable: String, _ arguments: [String], env: [String: String], stdin: Pipe? = nil) throws -> Lines {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.environment = env
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        if let stdin { process.standardInput = stdin }
        let lines = Lines()
        out.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
                if let value = try? JSONValue.decode(Data(line.utf8)) { lines.append(value) }
            }
        }
        try process.run()
        processes.append(process)
        return lines
    }

    func wait<T>(_ what: String, seconds: Double = 10, _ probe: () async -> T?) async throws -> T {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if let value = await probe() { return value }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        throw Timeout(what: what)
    }

    func cli(_ binary: String, _ arguments: [String], env: [String: String]) throws -> JSONValue {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: binary)
        process.arguments = ["mobile"] + arguments
        process.environment = env
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        try process.run()
        let data = out.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0, "mobile \(arguments)")
        return try JSONValue.decode(data)
    }

    func expectClosed(_ reason: CloseReason, _ body: () async throws -> Void, file: StaticString = #filePath, line: UInt = #line) async {
        do {
            try await body()
            XCTFail("expected close \(reason)", file: file, line: line)
        } catch GatewayError.closed(let got) {
            XCTAssertEqual(got, reason, file: file, line: line)
        } catch {
            XCTFail("expected close \(reason), got \(error)", file: file, line: line)
        }
    }

    func next(_ events: inout AsyncStream<ConnectionEvent>.Iterator, _ match: (JSONValue) -> Bool) async -> JSONValue? {
        while let event = await events.next() {
            if case .server(let frame) = event, match(frame) { return frame }
            if case .closed = event { return nil }
        }
        return nil
    }

    /// A send whose reply was lost is retried with the same message id and
    /// delivered once; a failed first message after create reuses the
    /// conversation instead of creating another.
    @MainActor
    func retries(_ client: GatewayClient) async throws {
        var outgoing = OutgoingMessage()
        let first = outgoing.id(text: "Once only")
        try await client.send(conversationId: conversation, text: "Once only", messageId: first)
        let retry = outgoing.id(text: "Once only")
        XCTAssertEqual(retry, first, "not confirmed yet: same id")
        try await client.send(conversationId: conversation, text: "Once only", messageId: retry)
        outgoing.sent()
        let read = try await client.call("conversationRead", ["conversationId": .string(conversation), "after": 0])
        let copies = (read["journal"]["events"].array ?? []).filter { $0["kind"] == "user" && $0["data"]["messageId"].string == first }
        XCTAssertEqual(copies.count, 1, "delivered once")

        var createdId: String?
        var firstMessage = OutgoingMessage()
        let before = try await client.refreshConversations().count
        do {
            _ = try await client.startConversation(createdId: &createdId, outgoing: &firstMessage, agentId: "claude", model: nil, toolAccess: nil, text: "FAIL ONCE then work")
            XCTFail("the first send was set up to fail")
        } catch GatewayError.remote {}
        let made = try XCTUnwrap(createdId, "the conversation was created")
        let id = try await client.startConversation(createdId: &createdId, outgoing: &firstMessage, agentId: "claude", model: nil, toolAccess: nil, text: "FAIL ONCE then work")
        XCTAssertEqual(id, made, "the retry reused it")
        let after = try await client.refreshConversations().count
        XCTAssertEqual(after, before + 1, "one conversation, not two")
        try await client.delete(conversationId: id)
    }

    func testPairDriveRespondPushRevoke() async throws {
        guard let binary = ProcessInfo.processInfo.environment["CHROME_USE_E2E_BIN"] else {
            throw XCTSkip("CHROME_USE_E2E_BIN is not set")
        }
        temp = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("cu-e2e-\(UUID().uuidString.prefix(8))")
        let relay = temp.appendingPathComponent("relay")
        let home = temp.appendingPathComponent("home")
        for dir in [temp!, home] {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        }
        let env = environment(relay: relay, home: home)
        let fixture = Bundle.module.url(forResource: "fake_panel", withExtension: "py", subdirectory: "Fixtures")!
        let control = Pipe()
        let panel = try launch("/usr/bin/env", ["python3", fixture.path, "--relay", relay.path, "--binary", binary], env: env, stdin: control)
        let ready = try await wait("fake panel") { panel.first { $0["ready"] == .bool(true) } }
        let pushURL = try XCTUnwrap(ready["push"].string)

        let gateway = try launch(binary, ["mobile", "serve", "--bind", "127.0.0.1:0", "--push-url", pushURL], env: env)
        let listening = try await wait("gateway") { gateway.first { $0["listening"].string != nil } }["listening"].string!
        let url = "ws://\(listening)/"

        let paired = try cli(binary, ["pair", "--url", url, "--name", "Simulator", "--json"], env: env)
        XCTAssertEqual(paired["profile"], "mobile-fixture")
        let link = try PairLink(parsing: try XCTUnwrap(paired["pairUri"].string))
        XCTAssertEqual(link.url.absoluteString, url)
        let computerName = try XCTUnwrap(link.computerName, "the link names the computer (n=)")
        XCTAssertEqual(link.suggestedName, computerName)

        // A wrong key fails; a pending device cannot skip pairing.
        let wrong = PairLink(url: link.url, deviceId: link.deviceId, pairSecret: Data(repeating: 9, count: 32), gatewayId: link.gatewayId)
        await expectClosed(.unauthorized) { _ = try await GatewayConnection.pair(link: wrong) { _ in } }
        await expectClosed(.unauthorized) { _ = try await GatewayConnection.connect(url: link.url, deviceId: link.deviceId, secret: link.pairSecret) }

        // Pair: the combined secret is stored before pairComplete leaves.
        var stored: Data?
        let (connection, secret) = try await GatewayConnection.pair(link: link) { stored = $0 }
        XCTAssertEqual(secret.count, 64)
        XCTAssertEqual(stored, secret)
        XCTAssertEqual(secret.prefix(32), link.pairSecret)
        XCTAssertEqual(connection.gatewayId, link.gatewayId)

        let hello = try await connection.request("hello")
        XCTAssertEqual(hello["computerName"].string, computerName)
        XCTAssertFalse((hello["gatewayVersion"].string ?? "").isEmpty)
        XCTAssertEqual(hello["pushMutes"], [])
        XCTAssertEqual(hello["fullAccessAllowed"], false)
        do {
            _ = try await connection.request("hello", ["name": "x"])
            XCTFail("hello takes no parameters")
        } catch GatewayError.remote(let code, _) {
            XCTAssertEqual(code, "denied")
        }

        let list = try await connection.request("conversationList")
        XCTAssertEqual(ConversationSummary.list(list).first?.id, conversation)

        // Denied operations come back as `denied`.
        for (op, params) in [("conversationSetAccess", ["conversationId": .string(conversation), "fullAccess": true] as JSONValue),
                             ("conversationCreate", ["agentId": "claude", "fullAccess": true]),
                             ("conversationCreate", ["agentId": "opencode"]),
                             ("serviceStop", [:])] {
            do {
                _ = try await connection.request(op, params)
                XCTFail("\(op) was allowed")
            } catch GatewayError.remote(let code, _) {
                XCTAssertEqual(code, "denied", op)
            }
        }

        var events = connection.events.makeAsyncIterator()
        let messageId = UUID().uuidString.lowercased()
        let image = Data(repeating: 0xAB, count: 2048)
        let put = try await connection.request("conversationAttachmentPut", ["conversationId": .string(conversation), "data": .string(image.base64EncodedString())])
        XCTAssertEqual(put["attachment"]["bytes"], 2048)
        let sent = try await connection.request("conversationSend", ["conversationId": .string(conversation), "messageId": .string(messageId), "text": "Summarize my inbox", "attachments": [put["attachment"]]])
        XCTAssertEqual(sent["message"]["accepted"], true)

        let subscribed = try await connection.request("subscribe", ["conversationId": .string(conversation), "after": 0])
        XCTAssertEqual(subscribed["subscribed"].string, conversation)
        let journalFound = await next(&events) { $0["event"] == "journal" && !($0["events"].array ?? []).isEmpty }
        let journal = try XCTUnwrap(journalFound)
        var timeline = Timeline()
        try timeline.apply(events: journal["events"].array!, cursor: journal["cursor"].int!)
        XCTAssertEqual(journal["cursor"], 1)
        guard case .user(_, _, let text, let images, _) = timeline.rows.first else { return XCTFail("no user row") }
        XCTAssertEqual(text, "Summarize my inbox")
        XCTAssertEqual(images, 1)

        // A token the Worker answers 410 for: the push is still attempted.
        let registered = try await connection.request("pushRegister", ["relayToken": "dead.e2e"])
        XCTAssertEqual(registered["registered"], true)
        // Let the push watcher record a baseline before the state changes.
        try await Task.sleep(nanoseconds: 4_500_000_000)
        control.fileHandleForWriting.write(Data("request\n".utf8))
        let requestFrameFound = await next(&events) { $0["event"] == "journal" && $0["events"][0]["kind"] == "request" }
        let requestFrame = try XCTUnwrap(requestFrameFound)
        try timeline.apply(events: requestFrame["events"].array!, cursor: requestFrame["cursor"].int!)
        XCTAssertEqual(ConversationSummary(requestFrame["conversation"]).phase, .waitingInput)
        let pending = try XCTUnwrap(timeline.openRequests.first)
        guard case .decisions(_, _, let options) = ApprovalForm.build(pending) else { return XCTFail("no decisions") }
        XCTAssertEqual(options.map(\.decision), ["allow", "deny"])

        // The push decrypts with the paired push key, as the NSE does it.
        let push = try await wait("push", seconds: 12) { panel.first { $0["push"].object != nil } }["push"]
        XCTAssertEqual(push["relayToken"].string, "dead.e2e")
        XCTAssertEqual(push["priority"], "high")
        XCTAssertEqual(push["collapseId"].string, conversation)
        let credential = GatewayCredential(url: link.url, deviceId: link.deviceId, secret: secret, gatewayId: link.gatewayId, name: "Mac", pairedAt: Date())
        let notice = try XCTUnwrap(PushNotice.open(payload: push["payload"].string!, credentials: [credential]))
        XCTAssertEqual(notice.code, .needsInput)
        XCTAssertEqual(notice.conversationId, conversation)
        XCTAssertEqual(notice.title, "Weekly report")

        // The Worker refused it: the live connection is told, and the same
        // token is refused when offered again (the app's client checks
        // that below, then registers a new one).
        let rejectedFound = await next(&events) { $0["event"] == "pushTokenRejected" }
        let rejected = try XCTUnwrap(rejectedFound)
        XCTAssertEqual(rejected["reason"], "unregistered")

        let answer = try await connection.request("conversationRespond", ApprovalForm.respondParams(conversationId: conversation, request: pending, decision: options[0].decision))
        XCTAssertEqual(answer["response"]["dispatched"], true)
        let resolvedFound = await next(&events) { $0["event"] == "journal" && ($0["events"].array ?? []).contains { $0["kind"] == "requestResolved" } }
        let resolved = try XCTUnwrap(resolvedFound)
        try timeline.apply(events: resolved["events"].array!, cursor: resolved["cursor"].int!)
        XCTAssertTrue(timeline.openRequests.isEmpty)

        // No browser session: reported, never started.
        do {
            _ = try await connection.request("screenshot", ["conversationId": .string(conversation)])
            XCTFail("screenshot without a session")
        } catch GatewayError.remote(let code, _) {
            XCTAssertEqual(code, "screenshot")
        }

        // The QR is spent; the link alone authenticates nobody.
        await expectClosed(.unauthorized) { _ = try await GatewayConnection.pair(link: link) { _ in } }
        await expectClosed(.unauthorized) { _ = try await GatewayConnection.connect(url: link.url, deviceId: link.deviceId, secret: link.pairSecret) }

        // A replayed frame closes the connection with 4400.
        let second = try await GatewayConnection.connect(url: link.url, deviceId: link.deviceId, secret: secret)
        _ = try await second.request("scheduleList")
        try await second.replayLastFrame()
        var secondEvents = second.events.makeAsyncIterator()
        var closedWith: CloseReason?
        while let event = await secondEvents.next() { if case .closed(let reason) = event { closedWith = reason } }
        XCTAssertEqual(closedWith, .malformed)

        // The app's client: resumes from its cursor across reconnects.
        let cache = InMemoryJournalCache()
        let client = await GatewayClient(credential: credential, cache: cache)
        // The client offers a refused token on connect and hears why; hello
        // names the computer and its version.
        let rejections = Lines()
        let saved = Lines()
        await MainActor.run {
            client.relayToken = "dead.e2e"
            client.onPushTokenRejected = { rejections.append(.string($0)) }
            client.onCredentialChanged = { saved.append(.string($0.name)) }
        }
        await client.start()
        let reason = try await wait("pushRegister refusal") { rejections.first { _ in true } }
        XCTAssertEqual(reason, "unregistered")
        _ = try await wait("hello") { saved.first { _ in true } }
        let named = await client.credential
        XCTAssertEqual(named.name, computerName)
        XCTAssertNotNil(named.gatewayVersion)
        // Mutes are per device and come back with hello on the next connection.
        try await client.setMuted(conversation, true)
        let mutedNow = await client.pushMutes
        XCTAssertEqual(mutedNow, [conversation])
        let probe = try await GatewayConnection.connect(url: link.url, deviceId: link.deviceId, secret: secret)
        let rehello = try await probe.request("hello")
        XCTAssertEqual(rehello["pushMutes"], [.string(conversation)])
        await probe.close()
        try await client.setMuted(conversation, false)
        let unmuted = await client.pushMutes
        XCTAssertTrue(unmuted.isEmpty)

        // Model listing for the create screen; cached on the gateway.
        let models = try await client.models(agentId: "claude")
        XCTAssertEqual(models.map(\.id), ["claude-opus-5-5", "claude-sonnet-5-5"])
        XCTAssertEqual(models.first?.isDefault, true)
        let again = try await client.models(agentId: "claude")
        XCTAssertEqual(again, models)
        let none = try await client.models(agentId: "gemini")
        XCTAssertTrue(none.isEmpty, "not inspectable: no request")

        // A conversation created on the desktop with full access: the phone
        // may stop it but not drive it until the computer allows this phone.
        let elevated = "5e6f7a8b-9c0d-4e1f-a2b3-c4d5e6f7a8b9"
        do {
            try await client.send(conversationId: elevated, text: "Ship it")
            XCTFail("an elevated conversation needs mobile access <id> full")
        } catch GatewayError.remote(let code, let message) {
            XCTAssertEqual(code, "accessTooHigh")
            XCTAssertTrue(message.contains("mobile access"), message)
        }
        try await client.stopTurn(conversationId: elevated)
        // Pi only with browser access; a model that reads as a flag is refused locally.
        do {
            _ = try await client.create(agentId: "pi", model: nil, toolAccess: .writeFiles)
            XCTFail("pi above browserOnly")
        } catch GatewayError.remote(let code, _) { XCTAssertEqual(code, "denied") }
        do {
            _ = try await client.create(agentId: "claude", model: "--dangerously-skip-permissions", toolAccess: nil)
            XCTFail("model starting with -")
        } catch GatewayError.remote(let code, _) { XCTAssertEqual(code, "invalidParams") }
        // The gateway itself refuses both too.
        do {
            _ = try await client.call("conversationCreate", ["agentId": "pi", "options": ["toolAccess": "writeFiles"]])
            XCTFail("gateway allowed pi above browserOnly")
        } catch GatewayError.remote(let code, _) { XCTAssertEqual(code, "denied") }
        do {
            _ = try await client.call("conversationCreate", ["agentId": "claude", "model": "-x"])
            XCTFail("gateway allowed a model starting with -")
        } catch GatewayError.remote(let code, _) { XCTAssertEqual(code, "invalidParams") }

        // Allowed on the computer: hello says so on the next connection, the
        // app records it (and requires its lock), and the send goes through.
        let access = try cli(binary, ["access", link.deviceId, "full", "--json"], env: env)
        XCTAssertEqual(access["fullAccessAllowed"], true)
        await client.stop()
        await client.start()
        _ = try await wait("hello with fullAccessAllowed") { await client.credential.fullAccessAllowed ? true : nil }
        try await client.send(conversationId: elevated, text: "Ship it")
        _ = try cli(binary, ["access", link.deviceId, "limited", "--json"], env: env)
        await client.stop()
        await client.start()
        _ = try await wait("hello back to limited") { await client.credential.fullAccessAllowed ? nil : true }

        // The app's answer to the refusal: a new token, which is stored.
        await MainActor.run { client.relayToken = "eyJwIjoiaW9zIn0.c2ln" }
        _ = try await wait("new token stored") { () async -> Bool? in
            let devices = try? self.cli(binary, ["devices", "--json"], env: env)
            return devices?["devices"][0]["push"] == .bool(true) ? true : nil
        }
        await client.subscribe(conversation)
        _ = try await wait("client journal") { await client.cursor(of: conversation) >= 4 ? true : nil }
        let rowsBefore = await client.timelines[conversation]!.rows.count
        await client.stop()
        control.fileHandleForWriting.write(Data("reply\n".utf8))
        await client.start()
        let resumed = try await wait("resumed journal") { () async -> Int64? in
            let cursor = await client.cursor(of: conversation)
            return cursor >= 7 ? cursor : nil
        }
        XCTAssertEqual(resumed, 7)
        let rowsAfter = await client.timelines[conversation]!.rows
        XCTAssertEqual(rowsAfter.count, rowsBefore + 1, "only the new answer is added")
        guard case .assistant(_, let final, _) = rowsAfter.last else { return XCTFail("no answer row") }
        XCTAssertEqual(final, "Done.")
        let schedules = try await client.schedules()
        XCTAssertEqual(schedules.first?.enabled, true)
        try await client.setSchedule(schedules[0].id, enabled: false)
        let toggled = try await client.schedules()
        XCTAssertEqual(toggled.first?.enabled, false)
        let created = try await client.create(agentId: "claude", model: nil, toolAccess: .writeFiles)
        XCTAssertEqual(created.toolAccess, .writeFiles)
        try await client.delete(conversationId: created.id)
        try await retries(client)

        // Revoking drops the live connection with 4403 within a second or so.
        let started = Date()
        let revoked = try cli(binary, ["revoke", link.deviceId, "--json"], env: env)
        XCTAssertEqual(revoked["revoked"].string, link.deviceId)
        let failed = try await wait("revocation", seconds: 5) { () async -> CloseReason? in
            if case .failed(let reason) = await client.status { return reason }
            return nil
        }
        XCTAssertEqual(failed, .revoked)
        XCTAssertLessThan(Date().timeIntervalSince(started), 3)
        XCTAssertTrue(failed.offersRepair)
        await expectClosed(.unauthorized) { _ = try await GatewayConnection.connect(url: link.url, deviceId: link.deviceId, secret: secret) }
        await connection.close()
    }
}
#endif
