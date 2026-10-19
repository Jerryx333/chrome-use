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
    @State private var models: [AgentModel] = []
    @State private var modelChoice = ""
    @State private var loadingModels = false
    @State private var modelsNote: String?
    private static let customChoice = "\u{0}custom"
    @State private var access: ToolAccess = .browserOnly
    @State private var message = ""
    @State private var loading = true
    @State private var working = false
    @State private var error: String?
    /// Created on an earlier Start whose first message failed: retrying
    /// sends to it instead of creating another conversation.
    @State private var createdId: String?
    @State private var outgoing = OutgoingMessage()

    private var choices: [ToolAccess] { ToolAccess.phoneCreateChoices(agentId: agentId) }
    private var modelValid: Bool { AgentModel.isValidName(chosenModel) }

    var body: some View {
        NavigationStack {
            Form {
                Section("Agent") {
                    if loading {
                        ProgressView()
                    } else {
                        Picker("Agent", selection: $agentId) {
                            ForEach(agents.filter(\.creatableFromPhone)) { agent in
                                Text(agent.installed ? agent.name : String(localized: "\(agent.name) (not installed)")).tag(agent.id)
                            }
                        }
                        .onChange(of: agentId) { _, id in
                            access = agents.first { $0.id == id }?.defaultAccess ?? .browserOnly
                            Task { await loadModels(id) }
                        }
                        // Once created, a retry only resends the first message.
                        .disabled(createdId != nil)
                    }
                    if agents.contains(where: { !$0.creatableFromPhone }) {
                        Text("OpenCode always runs commands, so it can only be started on the computer.").font(.caption).foregroundStyle(.secondary)
                    }
                }
                Section {
                    if loadingModels {
                        HStack { Text("Model"); Spacer(); ProgressView() }
                    } else if !models.isEmpty {
                        Picker("Model", selection: $modelChoice) {
                            Text("Default").tag("")
                            ForEach(models) { option in
                                Text(option.isDefault ? String(localized: "\(option.label) (default)") : option.label).tag(option.id)
                            }
                            Text("Other…").tag(Self.customChoice)
                        }
                        .disabled(createdId != nil)
                    }
                    if models.isEmpty || modelChoice == Self.customChoice {
                        TextField("Model (blank for default)", text: $model)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .disabled(createdId != nil)
                    }
                } footer: {
                    VStack(alignment: .leading) {
                        if !modelValid { Text("A model name cannot start with “-”.").foregroundStyle(.red) }
                        if let modelsNote { Text(modelsNote) }
                        Text("The model is chosen when the conversation is created and cannot be changed later.")
                    }
                }
                Section {
                    Picker("Access", selection: $access) {
                        ForEach(choices, id: \.self) { Text($0.label).tag($0) }
                    }
                    .disabled(createdId != nil)
                } footer: {
                    if agentId == "pi" {
                        Text("Pi conversations started from the phone can only use the browser.")
                    } else {
                        Text("Conversations started from the phone can at most write files; when the agent needs to run a command it asks you to approve.")
                    }
                }
                Section("First message") {
                    TextField("What should the agent do?", text: $message, axis: .vertical)
                        .lineLimit(3...8)
                }
                if let error {
                    Section { Text(error).foregroundStyle(.red) }
                }
            }
            .disabled(working)
            .navigationTitle("New conversation")
            .accessibilityIdentifier("newConversationForm")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.accessibilityIdentifier("cancel") }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Start") { Task { await create() } }
                        .disabled(working || agentId.isEmpty || !modelValid || message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
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
            if let first { await loadModels(first.id) }
        } catch {
            self.error = describe(error, deviceId: client.id)
        }
        loading = false
    }

    /// The agent CLI's model list (`agentInspect`); the gateway lets it run
    /// once per agent every 30 s, so a refusal falls back to typing a name.
    private func loadModels(_ id: String) async {
        models = []
        modelChoice = ""
        modelsNote = nil
        guard AgentModel.inspectable.contains(id) else { return }
        loadingModels = true
        defer { loadingModels = false }
        do {
            let found = try await client.models(agentId: id)
            guard id == agentId else { return }
            models = found
        } catch GatewayError.remote("rateLimited", _) {
            modelsNote = String(localized: "The model list is being read; try again shortly, or type a model name.")
        } catch {
            modelsNote = String(localized: "Cannot read the model list: \(describe(error, deviceId: client.id))")
        }
    }

    private var chosenModel: String {
        modelChoice.isEmpty || modelChoice == Self.customChoice ? model.trimmingCharacters(in: .whitespaces) : modelChoice
    }

    private func create() async {
        working = true
        defer { working = false }
        do {
            var made = createdId
            var pending = outgoing
            defer { createdId = made; outgoing = pending }
            let id = try await client.startConversation(
                createdId: &made, outgoing: &pending, agentId: agentId, model: chosenModel, toolAccess: access,
                text: message.trimmingCharacters(in: .whitespacesAndNewlines)
            )
            dismiss()
            created(id)
        } catch {
            self.error = describe(error, deviceId: client.id)
        }
    }
}
