import Foundation

/// One button on an approval card. `decision` is sent as is in
/// `conversationRespond`; the panel service builds the backend reply from
/// it (`agent_panel/approval.rs`, `reply_for_decision`).
public struct DecisionOption: Identifiable, Hashable, Sendable {
    public enum Kind: Hashable, Sendable {
        case allowOnce, allowForSession, allowTurn, deny, decline, cancel
        /// An OpenCode permission option, labelled by the agent.
        case agentOption(name: String, allows: Bool)
    }

    public let kind: Kind
    public let decision: JSONValue
    public var id: String { decision.encodedString() }

    public init(kind: Kind, decision: JSONValue) {
        self.kind = kind
        self.decision = decision
    }

    /// Lets the requested action run; disabled once Stop was requested.
    public var allows: Bool {
        switch kind {
        case .allowOnce, .allowForSession, .allowTurn: return true
        case .agentOption(_, let allows): return allows
        default: return false
        }
    }
}

public struct Question: Hashable, Sendable {
    public struct Option: Hashable, Sendable {
        public let label: String
        public let description: String?
    }

    public let text: String
    public let options: [Option]
    public let multiSelect: Bool
    public let allowsCustom: Bool
}

/// One question's answer, by option position: the shown labels may be
/// redacted, the service maps positions back to the originals.
public enum QuestionAnswer: Hashable, Sendable {
    case option(Int)
    case options([Int])
    case custom(String)

    var json: JSONValue {
        switch self {
        case .option(let i): return ["option": .int(Int64(i))]
        case .options(let list): return ["options": .array(list.map { .int(Int64($0)) })]
        case .custom(let text): return ["custom": .string(text)]
        }
    }
}

/// How the phone renders a request and which answers it may send.
public enum ApprovalForm: Hashable, Sendable {
    /// Buttons, each one a complete decision.
    case decisions(summary: String?, detail: String?, options: [DecisionOption])
    /// Questions answered together (`answersByPosition`); `deny` is the
    /// optional refusal button.
    case questions([Question], deny: DecisionOption?)
    /// The redacted request was cut short; answer on the computer.
    case truncated
    /// A backend or reply form this build does not know; answer on the computer.
    case unsupported(method: String)

    public static func build(_ request: PendingRequest) -> ApprovalForm {
        let params = request.params
        guard request.generation != nil, request.requestRevision != nil else {
            // The phone only sends bound replies; the legacy `result` form is refused.
            return .unsupported(method: request.method)
        }
        if params["truncated"] == .bool(true) { return .truncated }
        let word: (String) -> JSONValue = { .string($0) }
        switch request.method {
        case "can_use_tool" where params["tool_name"] == "AskUserQuestion":
            let questions = (params["input"]["questions"].array ?? []).map { q in
                Question(
                    text: q["question"].string ?? q["header"].string ?? "",
                    options: options(q), multiSelect: q["multiSelect"] == .bool(true),
                    allowsCustom: true
                )
            }
            return .questions(questions, deny: DecisionOption(kind: .deny, decision: word("deny")))
        case "can_use_tool":
            return .decisions(
                summary: params["tool_name"].string, detail: params["input"].isNull ? "{}" : params["input"].pretty(),
                options: [
                    DecisionOption(kind: .allowOnce, decision: word("allow")),
                    DecisionOption(kind: .deny, decision: word("deny")),
                ]
            )
        case "item/tool/requestUserInput":
            let questions = (params["questions"].array ?? []).map { q in
                let choices = options(q)
                return Question(
                    text: q["question"].string ?? q["header"].string ?? "",
                    options: choices, multiSelect: q["multiSelect"] == .bool(true),
                    allowsCustom: q["isOther"] == .bool(true) || choices.isEmpty
                )
            }
            return .questions(questions, deny: nil)
        case "item/commandExecution/requestApproval", "item/fileChange/requestApproval":
            var detail = params["command"].string
            if let cwd = params["cwd"].string { detail = [detail, "cwd: \(cwd)"].compactMap { $0 }.joined(separator: "\n") }
            return .decisions(
                summary: params["reason"].string, detail: detail,
                options: [
                    DecisionOption(kind: .allowOnce, decision: word("accept")),
                    DecisionOption(kind: .allowForSession, decision: word("acceptForSession")),
                    DecisionOption(kind: .decline, decision: word("decline")),
                    DecisionOption(kind: .cancel, decision: word("cancel")),
                ]
            )
        case "mcpServer/elicitation/request":
            var options: [DecisionOption] = []
            var detail: String?
            if safeToolApproval(params) {
                detail = params["_meta"]["tool_params"].pretty()
                options.append(DecisionOption(kind: .allowOnce, decision: word("accept")))
            }
            options.append(DecisionOption(kind: .decline, decision: word("decline")))
            options.append(DecisionOption(kind: .cancel, decision: word("cancel")))
            return .decisions(summary: params["message"].string, detail: detail, options: options)
        case "session/request_permission":
            var options: [DecisionOption] = (params["options"].array ?? []).compactMap { option in
                guard let id = option["optionId"].string else { return nil }
                return DecisionOption(
                    kind: .agentOption(name: option["name"].string ?? id, allows: option["kind"].string?.hasPrefix("allow") ?? false),
                    decision: ["optionId": .string(id)]
                )
            }
            options.append(DecisionOption(kind: .cancel, decision: word("cancel")))
            return .decisions(summary: params["toolCall"]["title"].string, detail: params["toolCall"]["rawInput"].pretty(), options: options)
        case "item/permissions/requestApproval":
            // The service accepts only `accept` here; Stop ends the turn instead.
            return .decisions(
                summary: params["reason"].string, detail: params["permissions"].pretty(),
                options: [DecisionOption(kind: .allowTurn, decision: word("accept"))]
            )
        default:
            return .unsupported(method: request.method)
        }
    }

    private static func options(_ question: JSONValue) -> [Question.Option] {
        (question["options"].array ?? []).map {
            Question.Option(label: $0["label"].string ?? "", description: $0["description"].string)
        }
    }

    /// Only Codex's chrome_use tool-call confirmation (an empty form) can be
    /// accepted with empty content; any other form needs fields the phone
    /// does not collect (as in the side panel).
    static func safeToolApproval(_ params: JSONValue) -> Bool {
        let schema = params["requestedSchema"]
        guard params["serverName"] == "chrome_use", params["mode"] == "form",
              params["_meta"]["codex_approval_kind"] == "mcp_tool_call",
              schema["type"] == "object",
              let properties = schema["properties"].object, properties.isEmpty,
              params["_meta"]["tool_params"].object != nil
        else { return false }
        switch schema["required"] {
        case .null: return true
        case .array(let list): return list.isEmpty
        default: return false
        }
    }

    /// The `decision` for answered questions.
    public static func answers(_ answers: [QuestionAnswer]) -> JSONValue {
        ["answersByPosition": .array(answers.map(\.json))]
    }

    /// `conversationRespond` params; all five fields are required.
    public static func respondParams(conversationId: String, request: PendingRequest, decision: JSONValue) -> JSONValue {
        [
            "conversationId": .string(conversationId),
            "requestId": request.requestId,
            "generation": request.generation.map(JSONValue.int) ?? .null,
            "requestRevision": request.requestRevision.map(JSONValue.int) ?? .null,
            "decision": decision,
        ]
    }
}
