import Foundation

/// A pending backend request (journal `request` event), bound to the
/// generation and revision it was shown for.
public struct PendingRequest: Identifiable, Hashable, Sendable {
    public let key: String
    public let requestId: JSONValue
    public let method: String
    public let params: JSONValue
    public let generation: Int64?
    public let requestRevision: Int64?
    public var sent: Bool
    public var id: String { key }
}

public enum TimelineRow: Identifiable, Hashable, Sendable {
    case user(key: String, messageId: String, text: String, images: Int, at: Date?)
    case assistant(key: String, text: String, role: String?)
    case tool(key: String, name: String, hint: String, failed: Bool, completed: Bool, browser: Bool)
    case note(key: String, text: String, failed: Bool)

    public var id: String {
        switch self {
        case .user(let key, _, _, _, _), .assistant(let key, _, _), .tool(let key, _, _, _, _, _), .note(let key, _, _):
            return key
        }
    }
}

public enum TimelineError: Error, Equatable {
    case historyGap(expected: Int64, got: Int64)
    case chunksOutOfOrder
    case cursorMismatch(expected: Int64, got: Int64)
}

/// The phone's projection of a conversation journal, following the side
/// panel's `PanelTimeline` (`extensions/ab-connect/panel-state.js`) for the
/// parts a phone shows: user messages, streamed and final text, compact tool
/// steps, error notes and open requests. All of it is display data.
public struct Timeline: Sendable {
    public private(set) var cursor: Int64 = 0
    public private(set) var rows: [TimelineRow] = []
    public private(set) var requests: [PendingRequest] = []
    private var index: [String: Int] = [:]
    private var textParts: [String: Int] = [:]
    private var requestKeys: [String: String] = [:]
    private var lastUser: String?

    public init() {}

    /// Apply one journal page. Events at or below the cursor are skipped (a
    /// resumed subscription may repeat them); anything else must be exactly
    /// the next sequence.
    public mutating func apply(events: [JSONValue], cursor next: Int64) throws {
        for event in events {
            guard let sequence = event["sequence"].int else { continue }
            if sequence <= cursor { continue }
            guard sequence == cursor + 1 else { throw TimelineError.historyGap(expected: cursor + 1, got: sequence) }
            try applyOne(event, sequence: sequence)
            cursor = sequence
        }
        guard next == cursor else { throw TimelineError.cursorMismatch(expected: cursor, got: next) }
    }

    public var openRequests: [PendingRequest] { requests.filter { !$0.sent } }

    private mutating func upsert(_ row: TimelineRow) {
        if let at = index[row.id] { rows[at] = row } else { index[row.id] = rows.count; rows.append(row) }
    }

    private mutating func applyOne(_ event: JSONValue, sequence: Int64) throws {
        let data = event["data"]
        let kind = event["kind"].string ?? ""
        let messageId = data["messageId"].string
        switch kind {
        case "user":
            let id = messageId ?? "seq\(sequence)"
            upsert(.user(
                key: "user:\(id)", messageId: id, text: data["text"].string ?? "",
                images: data["attachments"].array?.count ?? 0,
                at: event["at"].int.map(Date.init(milliseconds:))
            ))
            lastUser = id
        case "queueEdited":
            if let old = messageId, let at = index["user:\(old)"], case .user(let key, _, _, let images, let date) = rows[at] {
                rows[at] = .user(key: key, messageId: data["replacementId"].string ?? old, text: data["text"].string ?? "", images: images, at: date)
            }
        case "textDelta", "textFinal":
            let key = "text:\(messageId ?? "legacy"):\(data["itemId"].string ?? data["itemId"].int.map(String.init) ?? "")"
            var text = ""
            var role: String? = data["role"].string
            if let at = index[key], case .assistant(_, let existing, let oldRole) = rows[at] {
                text = existing
                role = role ?? oldRole
            }
            if kind == "textFinal" {
                let part = Int(data["part"].int ?? 0)
                if part == 0 { text = ""; textParts[key] = -1 }
                guard part == (textParts[key] ?? -1) + 1 else { throw TimelineError.chunksOutOfOrder }
                textParts[key] = part
            }
            text += data["text"].string ?? ""
            upsert(.assistant(key: key, text: text, role: role))
        case "textRole":
            let key = "text:\(messageId ?? "legacy"):\(data["itemId"].string ?? "")"
            if let at = index[key], case .assistant(_, let text, _) = rows[at] {
                rows[at] = .assistant(key: key, text: text, role: data["role"].string)
            }
        case "tool":
            let item = data["item"]
            guard ToolDescription.shown(item) else { break }
            let itemId = item["id"].string ?? item["tool_use_id"].string ?? "\(sequence)"
            let key = "tool:\(messageId ?? "legacy"):\(itemId)"
            upsert(.tool(
                key: key, name: ToolDescription.name(item), hint: ToolDescription.hint(item),
                failed: item["is_error"] == .bool(true) || item["status"] == "failed" || !(item["error"].isNull || item["error"] == .bool(false)),
                completed: data["completed"] == .bool(true),
                browser: ToolDescription.browserTool(item["tool"].string ?? item["name"].string) != nil
            ))
        case "request":
            let id = data["requestId"].encodedString()
            let revision = data["requestRevision"].int
            let key = revision.map { "\(id)#\($0)" } ?? id
            if let previous = requestKeys[id], previous != key { requests.removeAll { $0.key == previous } }
            requestKeys[id] = key
            let request = PendingRequest(
                key: key, requestId: data["requestId"], method: data["method"].string ?? "",
                params: data["params"], generation: data["generation"].int, requestRevision: revision, sent: false
            )
            if let at = requests.firstIndex(where: { $0.key == key }) { requests[at] = request } else { requests.append(request) }
        case "replySent":
            let key = requestKey(data["requestId"])
            if let at = requests.firstIndex(where: { $0.key == key }) { requests[at].sent = true }
        case "requestResolved":
            let id = data["requestId"].encodedString()
            let key = requestKey(data["requestId"])
            requests.removeAll { $0.key == key }
            requestKeys.removeValue(forKey: id)
        case "terminal":
            requests.removeAll()
            requestKeys.removeAll()
            let status = data["status"].string
            if status != "completed" {
                let text = data["error"].string ?? (status == "interrupted" ? "interrupted" : "failed")
                upsert(.note(key: "note:\(sequence)", text: ToolDescription.readableError(text), failed: status == "failed"))
            }
        case "exited", "processUnconfirmed":
            requests.removeAll()
            requestKeys.removeAll()
            if let error = data["error"].string {
                upsert(.note(key: "note:\(sequence)", text: ToolDescription.readableError(error), failed: true))
            }
        default:
            break
        }
    }

    private func requestKey(_ requestId: JSONValue) -> String {
        let id = requestId.encodedString()
        return requestKeys[id] ?? id
    }
}

/// Compact tool step labels, after `panel-state.js` (`toolName`, `toolHint`).
public enum ToolDescription {
    static let hiddenItems: Set<String> = ["reasoning", "userMessage", "agentMessage", "plan", "contextCompaction"]

    public static func shown(_ item: JSONValue) -> Bool {
        if item["truncated"] == .bool(true) { return true }
        if let type = item["type"].string, hiddenItems.contains(type) { return false }
        return item["tool"].string != nil || item["name"].string != nil || item["type"].string != nil
    }

    /// `chrome_use_open`, `mcp__chrome_use__chrome_use_open` → "open".
    public static func browserTool(_ name: String?) -> String? {
        guard var name else { return nil }
        if name.hasPrefix("mcp__chrome_use__") { name.removeFirst("mcp__chrome_use__".count) }
        guard name.hasPrefix("chrome_use_") || name.hasPrefix("chrome_use.") else { return nil }
        let tool = String(name.dropFirst("chrome_use_".count))
        guard !tool.isEmpty, tool.allSatisfy({ $0.isLowercase || $0 == "_" }) else { return nil }
        return tool.replacingOccurrences(of: "_", with: " ")
    }

    /// A stable label; the app localizes the known ones by prefix.
    public static func name(_ item: JSONValue) -> String {
        if let browser = browserTool(item["tool"].string ?? item["name"].string) { return "browser:\(browser)" }
        switch item["type"].string {
        case "commandExecution": return "command"
        case "fileChange": return "fileChange"
        case "webSearch": return "webSearch"
        case "tool_result": return "toolResult"
        default: return item["tool"].string ?? item["name"].string ?? item["type"].string ?? "tool"
        }
    }

    public static func hint(_ item: JSONValue) -> String {
        if item["type"] == "commandExecution", let command = item["command"].string {
            return String(command.replacingOccurrences(of: #"^/bin/\w+ -lc "#, with: "", options: .regularExpression).prefix(80))
        }
        var args = item["arguments"]
        if args.isNull { args = item["input"] }
        if args.isNull { args = item["rawInput"] }
        for key in ["url", "action", "query", "ref", "selector", "text", "tabId", "path", "command", "code"] {
            if let value = args[key].string?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty {
                return String(value.split(separator: "\n", omittingEmptySubsequences: false).first.map(String.init)?.prefix(80) ?? "")
            }
        }
        return ""
    }

    /// Agent CLIs often fail with raw API JSON; show its message.
    public static func readableError(_ text: String) -> String {
        if let start = text.firstIndex(of: "{"),
           let parsed = try? JSONValue.decode(Data(text[start...].utf8)) {
            for candidate in [parsed["error"]["message"], parsed["message"], parsed["error"]] {
                if let message = candidate.string?.trimmingCharacters(in: .whitespacesAndNewlines), !message.isEmpty {
                    return message
                }
            }
        }
        return text
    }
}
