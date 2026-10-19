import ChromeUseKit
import SwiftUI

/// A backend request rendered from its redacted projection, offering exactly
/// the decisions the panel service accepts for that method.
struct ApprovalCard: View {
    @Environment(AppModel.self) private var model
    let client: GatewayClient
    let conversationId: String
    let request: PendingRequest
    let stopRequested: Bool

    @State private var answering = false
    @State private var sent = false
    @State private var error: String?
    @State private var picked: [Int: Set<Int>] = [:]
    @State private var custom: [Int: String] = [:]

    var body: some View {
        let form = ApprovalForm.build(request)
        VStack(alignment: .leading, spacing: 10) {
            Label(isQuestion(form) ? LocalizedStringKey("需要你回答") : LocalizedStringKey("需要你确认"), systemImage: "hand.raised.fill")
                .font(.headline)
                .foregroundStyle(.orange)
            switch form {
            case .decisions(let summary, let detail, let options):
                if let summary { Text(summary).font(.callout.bold()) }
                if let detail, !detail.isEmpty {
                    ScrollView {
                        Text(detail).font(.caption.monospaced()).textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: 160)
                    .padding(8)
                    .background(Color.secondary.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
                }
                FlowButtons(options: options, disabled: answering || sent, stopRequested: stopRequested) { option in
                    Task { await respond(option.decision, as: .answer(option)) }
                }
            case .questions(let questions, let deny):
                ForEach(Array(questions.enumerated()), id: \.offset) { index, question in
                    QuestionField(question: question, picked: Binding(get: { picked[index] ?? [] }, set: { picked[index] = $0 }), custom: Binding(get: { custom[index] ?? "" }, set: { custom[index] = $0 }))
                }
                HStack {
                    Button("发送回答") { Task { await answer(questions) } }
                        .buttonStyle(.borderedProminent)
                        .disabled(answering || sent || stopRequested)
                    if let deny {
                        Button(deny.label) { Task { await respond(deny.decision, as: .answer(deny)) } }
                            .buttonStyle(.bordered)
                            .disabled(answering || sent)
                    }
                }
            case .truncated:
                Text("请求内容太长，手机上只显示了一部分。请在电脑上处理。").font(.callout)
            case .unsupported(let method):
                Text("这个请求（\(method)）只能在电脑上处理。").font(.callout)
            }
            if stopRequested {
                Text("已请求停止：只能拒绝，不能再允许。").font(.caption).foregroundStyle(.secondary)
            }
            if sent { Label("已发送", systemImage: "checkmark").font(.caption).foregroundStyle(.green) }
            if let error { Text(error).font(.caption).foregroundStyle(.red) }
        }
        .padding(12)
        .background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color.orange.opacity(0.5)))
    }

    private func isQuestion(_ form: ApprovalForm) -> Bool {
        if case .questions = form { return true }
        return false
    }

    private func answer(_ questions: [Question]) async {
        var answers: [QuestionAnswer] = []
        for (index, question) in questions.enumerated() {
            let typed = (custom[index] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let choices = (picked[index] ?? []).sorted()
            if question.allowsCustom, !typed.isEmpty {
                answers.append(.custom(typed))
            } else if question.multiSelect, !choices.isEmpty {
                answers.append(.options(choices))
            } else if let first = choices.first {
                answers.append(.option(first))
            } else {
                error = String(localized: "每个问题都要回答。")
                return
            }
        }
        await respond(ApprovalForm.answers(answers), as: .answerQuestions)
    }

    /// Refusing (deny, decline, cancel) is always free; allowing is gated.
    private func respond(_ decision: JSONValue, as action: GatedAction) async {
        guard model.allow(action) else { return }
        answering = true
        defer { answering = false }
        do {
            try await client.respond(conversationId: conversationId, request: request, decision: decision)
            sent = true
            error = nil
        } catch {
            self.error = describe(error)
        }
    }
}

private struct FlowButtons: View {
    let options: [DecisionOption]
    let disabled: Bool
    let stopRequested: Bool
    let action: (DecisionOption) -> Void

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack { buttons }
            VStack(alignment: .leading) { buttons }
        }
    }

    @ViewBuilder private var buttons: some View {
        ForEach(options) { option in
            if option.allows {
                Button(option.label) { action(option) }
                    .buttonStyle(.borderedProminent)
                    .disabled(disabled || stopRequested)
            } else {
                Button(option.label, role: option.kind == .deny || option.kind == .decline ? .destructive : nil) { action(option) }
                    .buttonStyle(.bordered)
                    .disabled(disabled)
            }
        }
    }
}

private struct QuestionField: View {
    let question: Question
    @Binding var picked: Set<Int>
    @Binding var custom: String

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(question.text).font(.callout.bold())
            ForEach(Array(question.options.enumerated()), id: \.offset) { index, option in
                Button {
                    if question.multiSelect {
                        if picked.contains(index) { picked.remove(index) } else { picked.insert(index) }
                    } else {
                        picked = [index]
                    }
                } label: {
                    HStack(alignment: .top) {
                        Image(systemName: picked.contains(index) ? (question.multiSelect ? "checkmark.square.fill" : "largecircle.fill.circle") : (question.multiSelect ? "square" : "circle"))
                        VStack(alignment: .leading) {
                            Text(option.label)
                            if let description = option.description { Text(description).font(.caption).foregroundStyle(.secondary) }
                        }
                    }
                }
                .buttonStyle(.plain)
            }
            if question.allowsCustom {
                TextField("你的回答", text: $custom, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
            }
        }
    }
}
