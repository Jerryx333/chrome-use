import ChromeUseKit
import SwiftUI

struct ComputersView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        List {
            if model.clients.isEmpty {
                ContentUnavailableView {
                    Label("还没有配对的电脑", systemImage: "desktopcomputer")
                } description: {
                    Text("在电脑上运行 chrome-use mobile serve，再用 chrome-use mobile pair --url <地址> 显示二维码，然后在这里扫码。")
                } actions: {
                    Button("配对电脑") { model.pendingLink = nil; model.showPairing = true }
                        .buttonStyle(.borderedProminent)
                }
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
        .navigationTitle("电脑")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { model.showSettings = true } label: { Image(systemName: "gearshape") }
                    .accessibilityLabel("设置")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { model.pendingLink = nil; model.showPairing = true } label: { Image(systemName: "qrcode.viewfinder") }
                    .accessibilityLabel("配对电脑")
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
                    Text("\(waiting) 个对话等你处理").font(.caption.bold()).foregroundStyle(.orange)
                }
            }
        }
        .padding(.vertical, 4)
    }
}
