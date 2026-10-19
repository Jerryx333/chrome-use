import ChromeUseKit
import SwiftUI

/// Start a conversation from the phone. Tool access is capped at "write
/// files" and full access is never offered; OpenCode (which always runs
/// commands) is created on the computer only.
struct NewConversationView: View {
    @Environment(\.dismiss) private var dismiss
    let client: GatewayClient
    let created: (String) -> Void

    @State private var agents: [Agent] = []
    @State private var agentId = ""
    @State private var model = ""
    @State private var access: ToolAccess = .browserOnly
    @State private var message = ""
    @State private var loading = true
    @State private var working = false
    @State private var error: String?

    private var choices: [ToolAccess] { ToolAccess.allCases.filter { $0 <= ToolAccess.phoneCreateMaximum } }

    var body: some View {
        NavigationStack {
            Form {
                Section("Agent") {
                    if loading {
                        ProgressView()
                    } else {
                        Picker("Agent", selection: $agentId) {
                            ForEach(agents.filter(\.creatableFromPhone)) { agent in
                                Text(agent.installed ? agent.name : String(localized: "\(agent.name)（未安装）")).tag(agent.id)
                            }
                        }
                        .onChange(of: agentId) { _, id in
                            access = agents.first { $0.id == id }?.defaultAccess ?? .browserOnly
                        }
                    }
                    if agents.contains(where: { !$0.creatableFromPhone }) {
                        Text("OpenCode 总是可以运行命令，只能在电脑上新建。").font(.caption).foregroundStyle(.secondary)
                    }
                }
                Section {
                    TextField("模型（留空用默认）", text: $model)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } footer: {
                    Text("模型只能在新建时选择，之后不能更改。")
                }
                Section {
                    Picker("权限", selection: $access) {
                        ForEach(choices, id: \.self) { Text($0.label).tag($0) }
                    }
                } footer: {
                    Text("手机上新建的对话最多“可写文件”；需要运行命令时 agent 会请你审批。")
                }
                Section("第一条消息") {
                    TextField("要 agent 做什么？", text: $message, axis: .vertical)
                        .lineLimit(3...8)
                }
                if let error {
                    Section { Text(error).foregroundStyle(.red) }
                }
            }
            .navigationTitle("新建对话")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("取消") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("开始") { Task { await create() } }
                        .disabled(working || agentId.isEmpty || message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
            .task { await load() }
        }
    }

    private func load() async {
        do {
            agents = try await client.discover()
            let first = agents.first { $0.creatableFromPhone && $0.installed } ?? agents.first { $0.creatableFromPhone }
            agentId = first?.id ?? ""
            access = first?.defaultAccess ?? .browserOnly
        } catch {
            self.error = describe(error)
        }
        loading = false
    }

    private func create() async {
        working = true
        defer { working = false }
        do {
            let summary = try await client.create(agentId: agentId, model: model.trimmingCharacters(in: .whitespaces), toolAccess: access)
            try await client.send(conversationId: summary.id, text: message.trimmingCharacters(in: .whitespacesAndNewlines))
            dismiss()
            created(summary.id)
        } catch {
            self.error = describe(error)
        }
    }
}
