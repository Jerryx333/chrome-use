import ChromeUseKit
import SwiftUI

struct ConversationsView: View {
    @Environment(AppModel.self) private var model
    let client: GatewayClient
    @State private var error: String?
    @State private var deleting: ConversationSummary?
    @State private var creating = false

    var body: some View {
        List {
            ConnectionBanner(client: client)
            if client.conversations.isEmpty, client.status == .connected {
                ContentUnavailableView("没有对话", systemImage: "bubble.left.and.bubble.right", description: Text("点右上角新建一个对话。"))
                    .listRowBackground(Color.clear)
            }
            ForEach(client.conversations) { conversation in
                NavigationLink(value: Route.conversation(client.id, conversation.id)) {
                    ConversationRow(conversation: conversation)
                }
                .listRowBackground(conversation.phase == .waitingInput ? Color.orange.opacity(0.1) : nil)
                .swipeActions {
                    Button(role: .destructive) { deleting = conversation } label: { Label("删除", systemImage: "trash") }
                        .disabled(conversation.phase.isBusy)
                }
            }
            if let error {
                Text(error).font(.footnote).foregroundStyle(.red)
            }
        }
        .navigationTitle(client.credential.name)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                NavigationLink(value: Route.schedules(client.id)) { Image(systemName: "calendar.badge.clock") }
                    .accessibilityLabel("定时任务")
                Button {
                    if model.requireWrite() { creating = true }
                } label: { Image(systemName: "square.and.pencil") }
                    .accessibilityLabel("新建对话")
                    .disabled(client.status != .connected)
            }
        }
        .refreshable { await refresh() }
        .task {
            // The gateway pushes journals only for open conversations; the
            // list's phases are polled while it is on screen.
            while !Task.isCancelled {
                await refresh()
                try? await Task.sleep(nanoseconds: 3_000_000_000)
            }
        }
        .sheet(isPresented: $creating) {
            NewConversationView(client: client) { id in
                model.path.append(.conversation(client.id, id))
            }
        }
        .confirmationDialog("删除这个对话？", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible, presenting: deleting) { conversation in
            Button("删除", role: .destructive) {
                Task {
                    do { try await client.delete(conversationId: conversation.id) } catch { self.error = describe(error) }
                }
            }
        } message: { conversation in
            let title = conversation.title.isEmpty ? String(localized: "新对话") : conversation.title
            Text("“\(title)”会从电脑上删除，无法恢复。")
        }
    }

    private func refresh() async {
        guard client.status == .connected else { return }
        do {
            try await client.refreshConversations()
            error = nil
        } catch {
            self.error = describe(error)
        }
    }
}

struct ConnectionBanner: View {
    @Environment(AppModel.self) private var model
    let client: GatewayClient

    var body: some View {
        switch client.status {
        case .connected:
            EmptyView()
        case .failed(let reason):
            VStack(alignment: .leading, spacing: 8) {
                Label(reason.message, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red)
                HStack {
                    if reason.offersRepair {
                        Button("重新配对") { model.startRepair(client.id) }.buttonStyle(.borderedProminent)
                    }
                    Button("重试") { client.reconnectNow() }.buttonStyle(.bordered)
                }
            }
            .font(.callout)
        default:
            HStack {
                ProgressView()
                Text(client.status.label).font(.callout).foregroundStyle(.secondary)
                Spacer()
                Button("立即重试") { client.reconnectNow() }.font(.callout)
            }
        }
    }
}

struct PhaseBadge: View {
    let phase: Phase

    var body: some View {
        Text(phase.label)
            .font(.caption2.bold())
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(phase.tint.opacity(phase == .waitingInput ? 1 : 0.15), in: Capsule())
            .foregroundStyle(phase == .waitingInput ? Color.white : phase.tint)
    }
}

struct ConversationRow: View {
    let conversation: ConversationSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text(conversation.title.isEmpty ? String(localized: "新对话") : conversation.title)
                    .font(.headline)
                    .lineLimit(1)
                Spacer()
                PhaseBadge(phase: conversation.phase)
            }
            HStack(spacing: 6) {
                Text(conversation.agentId)
                if conversation.scheduleId != nil { Image(systemName: "clock") }
                Text(conversation.updated, format: .relative(presentation: .named))
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            if let preview = conversation.lastTurnPreview, !conversation.phase.isBusy {
                Text(preview).font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
            }
        }
        .padding(.vertical, 3)
    }
}
