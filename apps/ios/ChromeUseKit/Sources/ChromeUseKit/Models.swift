import Foundation

/// Conversation phases of the panel service (`conversation.rs`, `Phase`).
public enum Phase: String, Sendable, CaseIterable {
    case idle, starting, running, waitingInput, stopping, failed, interrupted, recoveryRequired
    case unknown

    public init(_ raw: String?) {
        self = raw.flatMap(Phase.init(rawValue:)) ?? .unknown
    }

    /// The gateway polls faster and the push watcher counts these as busy.
    public var isBusy: Bool {
        [.starting, .running, .waitingInput, .stopping].contains(self)
    }
}

/// Tool access, least to most authority (`policy.rs`, `access_rank`).
public enum ToolAccess: String, Sendable, CaseIterable, Comparable {
    case browserOnly, readFiles, writeFiles, runCommands

    var rank: Int { Self.allCases.firstIndex(of: self)! }
    public static func < (a: ToolAccess, b: ToolAccess) -> Bool { a.rank < b.rank }

    /// What a conversation runs with when it names none; mirrors
    /// `effective_rank`.
    public static func effective(agentId: String, toolAccess: ToolAccess?, fullAccess: Bool) -> ToolAccess {
        if let toolAccess { return toolAccess }
        if fullAccess || agentId == "opencode" { return .runCommands }
        if agentId == "codex" { return .readFiles }
        return .browserOnly
    }

    /// Beyond what a phone may start (`policy::elevated`): full access,
    /// commands (every OpenCode conversation), or Pi above `browserOnly`.
    /// The gateway refuses to drive these unless the computer allowed this
    /// phone full access.
    public static func elevated(agentId: String, toolAccess: ToolAccess?, fullAccess: Bool) -> Bool {
        let rank = effective(agentId: agentId, toolAccess: toolAccess, fullAccess: fullAccess)
        return fullAccess || rank > phoneCreateMaximum || (agentId == "pi" && rank > .browserOnly)
    }

    /// The most a conversation created from the phone may have.
    public static let phoneCreateMaximum: ToolAccess = .writeFiles

    /// What the create screen offers: up to `writeFiles`, and Pi (whose
    /// tools act beyond the browser) only `browserOnly`.
    public static func phoneCreateChoices(agentId: String) -> [ToolAccess] {
        agentId == "pi" ? [.browserOnly] : allCases.filter { $0 <= phoneCreateMaximum }
    }
}

public struct ConversationSummary: Identifiable, Hashable, Sendable {
    public let raw: JSONValue
    public var id: String { raw["id"].string ?? "" }
    public var agentId: String { raw["agentId"].string ?? "" }
    public var title: String {
        let title = raw["title"].string ?? ""
        return title.isEmpty ? "" : title
    }
    public var phase: Phase { Phase(raw["phase"].string) }
    public var updated: Date { Date(milliseconds: raw["updated"].int ?? raw["created"].int ?? 0) }
    public var model: String? { raw["model"].string ?? raw["requestedModel"].string }
    public var fullAccess: Bool { raw["fullAccess"].bool ?? false }
    public var toolAccess: ToolAccess? { raw["options"]["toolAccess"].string.flatMap(ToolAccess.init(rawValue:)) }
    public var effectiveAccess: ToolAccess { .effective(agentId: agentId, toolAccess: toolAccess, fullAccess: fullAccess) }
    public var elevated: Bool { ToolAccess.elevated(agentId: agentId, toolAccess: toolAccess, fullAccess: fullAccess) }
    public var stopRequested: Bool { raw["control"].string == "stopRequested" }
    public var scheduleId: String? { raw["scheduleId"].string }
    public var lastTurnOutcome: String? { raw["lastTurn"]["outcome"].string }
    public var lastTurnPreview: String? { raw["lastTurn"]["preview"].string }
    public var lastError: String? { raw["lastError"].string }
    public var queued: Int { Int(raw["queued"].int ?? 0) }

    public init(_ raw: JSONValue) { self.raw = raw }

    public static func list(_ result: JSONValue) -> [ConversationSummary] {
        (result["conversations"].array ?? []).map(ConversationSummary.init)
            .filter { !$0.id.isEmpty }
            .sorted { $0.updated > $1.updated }
    }
}

public struct Agent: Identifiable, Hashable, Sendable {
    public let id: String
    public let name: String
    public let installed: Bool

    /// Agents the phone may start a conversation with: OpenCode always runs
    /// commands, so it is created on the desktop only (spec, Decided).
    public var creatableFromPhone: Bool { id != "opencode" }
    public var defaultAccess: ToolAccess { .effective(agentId: id, toolAccess: nil, fullAccess: false) }

    public static func list(_ result: JSONValue) -> [Agent] {
        (result["agents"].array ?? []).compactMap { value in
            guard let id = value["id"].string else { return nil }
            return Agent(id: id, name: value["name"].string ?? id, installed: !value["executable"].isNull)
        }
    }
}

/// One entry of `agentInspect`'s model list.
public struct AgentModel: Identifiable, Hashable, Sendable {
    /// The gateway refuses model names that would read as CLI flags.
    public static func isValidName(_ name: String) -> Bool {
        !name.trimmingCharacters(in: .whitespaces).hasPrefix("-")
    }

    public let id: String
    public let label: String
    public let isDefault: Bool

    /// Agents the gateway can inspect for models.
    public static let inspectable: Set<String> = ["codex", "claude", "opencode", "pi"]

    public static func list(_ result: JSONValue) -> [AgentModel] {
        (result["inspection"]["models"].array ?? []).compactMap { value in
            guard let id = value["id"].string, !id.isEmpty else { return nil }
            return AgentModel(id: id, label: value["label"].string ?? id, isDefault: value["isDefault"] == .bool(true))
        }
    }
}

public struct Schedule: Identifiable, Hashable, Sendable {
    public let raw: JSONValue
    public var id: String { raw["id"].string ?? "" }
    public var name: String { raw["name"].string ?? "" }
    public var enabled: Bool { raw["enabled"].bool ?? false }
    public var agentId: String { raw["agentId"].string ?? "" }
    public var lastResult: String? { raw["lastResult"].string }
    public var lastChat: String? { raw["lastChat"].string }
    public var nextRun: Date? { raw["nextRun"].int.map(Date.init(milliseconds:)) }
    public var rule: JSONValue { raw["rule"] }
    public var running: Bool { raw["activeChat"].string != nil }
    public var elevated: Bool {
        ToolAccess.elevated(agentId: agentId, toolAccess: raw["options"]["toolAccess"].string.flatMap(ToolAccess.init(rawValue:)),
                            fullAccess: raw["fullAccess"].bool ?? false)
    }

    public init(_ raw: JSONValue) { self.raw = raw }

    public static func list(_ result: JSONValue) -> [Schedule] {
        (result["schedules"].array ?? []).map(Schedule.init).filter { !$0.id.isEmpty }
    }
}

extension Date {
    public init(milliseconds: Int64) {
        self.init(timeIntervalSince1970: TimeInterval(milliseconds) / 1000)
    }
}
