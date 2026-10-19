import ChromeUseKit
import SwiftUI

struct ComputersView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        List {
            if model.clients.isEmpty {
                ContentUnavailableView {
                    Label("No paired computers yet", systemImage: "desktopcomputer")
                } description: {
                    Text("On the computer, run chrome-use mobile serve, then chrome-use mobile pair --url <address> to show a QR code, and scan it here.")
                } actions: {
                    Button("Pair a computer") { model.pendingLink = nil; model.showPairing = true }
                        .buttonStyle(.borderedProminent)
                }
                .accessibilityIdentifier("noComputers")
                .listRowBackground(Color.clear)
            }
            ForEach(model.clients) { client in
                NavigationLink(value: Route.computer(client.id)) {
                    ComputerRow(client: client)
                }
            }
            if let error = model.storeError {
                Text(error).font(.footnote).foregroundStyle(.red)
            }
        }
        .navigationTitle("Computers")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { model.showSettings = true } label: { Image(systemName: "gearshape") }
                    .accessibilityLabel("Settings")
                    .accessibilityIdentifier("settings")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { model.pendingLink = nil; model.showPairing = true } label: { Image(systemName: "qrcode.viewfinder") }
                    .accessibilityLabel("Pair a computer")
            }
        }
        .refreshable { model.clients.forEach { $0.reconnectNow() } }
    }
}

struct ComputerRow: View {
    let client: GatewayClient

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer")
                .font(.title2)
                .foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 3) {
                Text(client.credential.name).font(.headline)
                HStack(spacing: 6) {
                    Circle().fill(client.status.color).frame(width: 8, height: 8)
                    Text(client.status.label).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
                let waiting = client.conversations.filter { $0.phase == .waitingInput }.count
                if waiting > 0 {
                    Text("\(waiting) waiting for you").font(.caption.bold()).foregroundStyle(.orange)
                }
            }
        }
        .padding(.vertical, 4)
    }
}
