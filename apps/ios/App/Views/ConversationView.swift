import ChromeUseKit
import PhotosUI
import SwiftUI

struct ConversationView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let client: GatewayClient
    let conversationId: String

    @State private var text = ""
    @State private var images: [PreparedImage] = []
    @State private var picks: [PhotosPickerItem] = []
    @State private var sending = false
    @State private var error: String?
    @State private var shot: GatewayClient.Screenshot?
    @State private var shooting = false
    @State private var confirmDelete = false
    @FocusState private var composing: Bool

    struct PreparedImage: Identifiable {
        let id = UUID()
        let jpeg: Data
        let thumbnail: UIImage
    }

    private var summary: ConversationSummary? {
        client.summaries[conversationId] ?? client.conversations.first { $0.id == conversationId }
    }
    private var timeline: Timeline { client.timelines[conversationId] ?? Timeline() }
    private var phase: Phase { summary?.phase ?? .unknown }
    private var queued: [JSONValue] { client.queuedMessages[conversationId] ?? [] }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    if client.ended.contains(conversationId) {
                        Label("This conversation was deleted on the computer.", systemImage: "trash").foregroundStyle(.secondary)
                    }
                    if let failure = client.subscriptionErrors[conversationId] {
                        Label(failure, systemImage: "exclamationmark.triangle").font(.footnote).foregroundStyle(.orange)
                    }
                    ForEach(timeline.rows) { row in
                        TimelineRowView(row: row)
                    }
                    if [.starting, .running].contains(phase), timeline.openRequests.isEmpty {
                        HStack(spacing: 8) { ProgressView(); Text(phase.label).foregroundStyle(.secondary) }
                            .font(.footnote)
                    }
                    ForEach(timeline.openRequests) { request in
                        ApprovalCard(client: client, conversationId: conversationId, request: request, stopRequested: summary?.stopRequested ?? false, elevated: summary?.elevated ?? false)
                            .id("request-\(request.key)")
                    }
                    if !queued.isEmpty {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("Queued messages").font(.caption.bold()).foregroundStyle(.secondary)
                            ForEach(Array(queued.enumerated()), id: \.offset) { _, message in
                                Text(message["text"].string ?? "")
                                    .font(.callout)
                                    .lineLimit(3)
                                    .padding(8)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .background(Color.secondary.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
                            }
                        }
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding()
            }
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: timeline.cursor) { _, _ in
                withAnimation { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onAppear { proxy.scrollTo("bottom", anchor: .bottom) }
        }
        .safeAreaInset(edge: .bottom) { composer }
        .navigationTitle(summary?.title.isEmpty == false ? summary!.title : String(localized: "Conversation"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(summary?.title.isEmpty == false ? summary!.title : String(localized: "Conversation")).font(.headline).lineLimit(1)
                    HStack(spacing: 4) {
                        if let summary { PhaseBadge(phase: summary.phase) }
                        if client.pushMutes.contains(conversationId) {
                            Image(systemName: "bell.slash.fill").font(.caption2).foregroundStyle(.secondary)
                                .accessibilityLabel("Muted")
                        }
                    }
                }
            }
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button { Task { await screenshot() } } label: {
                    if shooting { ProgressView() } else { Image(systemName: "camera.viewfinder") }
                }
                .accessibilityLabel("Screenshot")
                .disabled(shooting || client.status != .connected)
                Menu {
                    // Muting is free: it only stops this phone's pushes.
                    let muted = client.pushMutes.contains(conversationId)
                    Button(muted ? LocalizedStringKey("Unmute notifications") : LocalizedStringKey("Mute notifications"), systemImage: muted ? "bell" : "bell.slash") {
                        Task {
                            do { try await client.setMuted(conversationId, !muted) } catch { self.error = describe(error, deviceId: client.id) }
                        }
                    }
                    .disabled(client.status != .connected)
                    Button("Delete conversation", systemImage: "trash", role: .destructive) { confirmDelete = true }
                        .disabled(phase.isBusy)
                } label: { Image(systemName: "ellipsis.circle") }
            }
        }
        .onAppear { client.subscribe(conversationId) }
        .onDisappear { client.unsubscribe(conversationId) }
        .sheet(item: Binding(get: { shot.map(ShotItem.init) }, set: { if $0 == nil { shot = nil } })) { item in
            ScreenshotView(shot: item.shot)
        }
        .confirmationDialog("Delete this conversation?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive) {
                Task {
                    do { try await client.delete(conversationId: conversationId); dismiss() } catch { self.error = describe(error, deviceId: client.id) }
                }
            }
        } message: {
            Text("The conversation will be deleted from the computer. This cannot be undone.")
        }
        .onChange(of: picks) { _, items in Task { await load(items) } }
    }

    struct ShotItem: Identifiable {
        let id = UUID()
        let shot: GatewayClient.Screenshot
    }

    private var composer: some View {
        VStack(spacing: 6) {
            if let error {
                Text(error).font(.footnote).foregroundStyle(.red).frame(maxWidth: .infinity, alignment: .leading)
                    .onTapGesture { self.error = nil }
            }
            if !images.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack {
                        ForEach(images) { image in
                            Image(uiImage: image.thumbnail)
                                .resizable().scaledToFill()
                                .frame(width: 56, height: 56)
                                .clipShape(RoundedRectangle(cornerRadius: 8))
                                .overlay(alignment: .topTrailing) {
                                    Button { images.removeAll { $0.id == image.id } } label: {
                                        Image(systemName: "xmark.circle.fill").symbolRenderingMode(.palette).foregroundStyle(.white, .black.opacity(0.6))
                                    }
                                    .accessibilityLabel("Remove image")
                                }
                        }
                    }
                }
            }
            HStack(alignment: .bottom, spacing: 8) {
                PhotosPicker(selection: $picks, maxSelectionCount: max(1, ImagePreparer.maximumImages - images.count), matching: .images) {
                    Image(systemName: "photo.on.rectangle").font(.title3)
                }
                .disabled(images.count >= ImagePreparer.maximumImages || sending)
                .accessibilityLabel("Add images")
                TextField(phase.isBusy ? LocalizedStringKey("Add a message (queued)") : LocalizedStringKey("Message the agent"), text: $text, axis: .vertical)
                    .lineLimit(1...6)
                    .focused($composing)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(Color.secondary.opacity(0.12), in: RoundedRectangle(cornerRadius: 18))
                if phase.isBusy && text.isEmpty {
                    Button { Task { await stop() } } label: {
                        Image(systemName: "stop.circle.fill").font(.title)
                    }
                    .tint(.red)
                    .disabled(summary?.stopRequested == true)
                    .accessibilityLabel("Stop")
                } else {
                    Button { Task { await send() } } label: {
                        if sending { ProgressView() } else { Image(systemName: "arrow.up.circle.fill").font(.title) }
                    }
                    .disabled(sending || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || client.status != .connected || phase == .recoveryRequired)
                    .accessibilityLabel("Send")
                }
            }
        }
        .padding(.horizontal)
        .padding(.vertical, 8)
        .background(.bar)
    }

    private func load(_ items: [PhotosPickerItem]) async {
        guard !items.isEmpty else { return }
        picks = []
        for item in items where images.count < ImagePreparer.maximumImages {
            do {
                guard let data = try await item.loadTransferable(type: Data.self) else { continue }
                let jpeg = try await Task.detached { try ImagePreparer.jpeg(from: data) }.value
                if let thumbnail = UIImage(data: jpeg)?.preparingThumbnail(of: CGSize(width: 112, height: 112)) {
                    images.append(PreparedImage(jpeg: jpeg, thumbnail: thumbnail))
                }
            } catch {
                self.error = describe(error, deviceId: client.id)
            }
        }
    }

    private func send() async {
        if let refusal = model.elevatedRefusal(client, elevated: summary?.elevated ?? false) { error = refusal; return }
        guard model.allow(.send) else { return }
        let message = text.trimmingCharacters(in: .whitespacesAndNewlines)
        sending = true
        defer { sending = false }
        do {
            try await client.send(conversationId: conversationId, text: message, images: images.map(\.jpeg))
            text = ""
            images = []
            error = nil
        } catch {
            self.error = describe(error, deviceId: client.id)
        }
    }

    /// Stop is never paywalled.
    private func stop() async {
        do { try await client.stopTurn(conversationId: conversationId) } catch { self.error = describe(error, deviceId: client.id) }
    }

    private func screenshot() async {
        shooting = true
        defer { shooting = false }
        do { shot = try await client.screenshot(conversationId: conversationId) } catch { self.error = describe(error, deviceId: client.id) }
    }
}

struct TimelineRowView: View {
    let row: TimelineRow

    var body: some View {
        switch row {
        case .user(_, _, let text, let images, _):
            VStack(alignment: .trailing, spacing: 4) {
                Text(text)
                    .textSelection(.enabled)
                    .padding(10)
                    .background(Color.accentColor.opacity(0.15), in: RoundedRectangle(cornerRadius: 14))
                if images > 0 {
                    Label("\(images) images", systemImage: "photo").font(.caption).foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 40)
        case .assistant(_, let text, _):
            // Agent output is untrusted display data: plain text, links are not opened automatically.
            Text(text)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .tool(_, let name, let hint, let failed, let completed, _):
            HStack(spacing: 6) {
                Image(systemName: failed ? "xmark.octagon" : completed ? "checkmark.circle" : "gearshape.2")
                    .foregroundStyle(failed ? .red : completed ? .green : .secondary)
                Text(ToolLabel.text(name)).bold()
                if !hint.isEmpty { Text(hint).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle) }
            }
            .font(.caption)
            .padding(.vertical, 2)
        case .note(_, let text, let failed):
            Label(text, systemImage: failed ? "exclamationmark.triangle" : "info.circle")
                .font(.footnote)
                .foregroundStyle(failed ? .red : .secondary)
        }
    }
}
