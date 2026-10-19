import ChromeUseKit
import StoreKit
import SwiftUI

struct PaywallView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let store = model.store
        NavigationStack {
            ScrollView {
                VStack(spacing: 20) {
                    Image(systemName: "iphone.and.arrow.forward")
                        .font(.system(size: 54))
                        .foregroundStyle(.tint)
                        .padding(.top, 24)
                    Text("解锁 chrome-use 遥控").font(.title2.bold())
                    statusText(store.entitlement, clock: store.clock)
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 10) {
                        Label("在手机上发消息、新建对话", systemImage: "paperplane")
                        Label("随时处理 agent 的审批请求", systemImage: "hand.raised")
                        Label("停止任务、截图、运行定时任务", systemImage: "camera.viewfinder")
                        Label("一次付费，永久使用；看对话始终免费", systemImage: "checkmark.seal")
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
                    .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 14))
                    if let unlock = store.unlockProduct, store.entitlement != .unlocked {
                        Button {
                            Task { await store.buy(unlock) }
                        } label: {
                            Text("一次性解锁 \(unlock.displayPrice)").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.borderedProminent)
                        .controlSize(.large)
                        .disabled(store.purchasing)
                    }
                    if let trial = store.trialProduct, store.entitlement == .trialNotStarted {
                        Button {
                            Task { await store.buy(trial) }
                        } label: {
                            Text("免费试用 14 天").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.large)
                        .disabled(store.purchasing)
                    }
                    if store.unlockProduct == nil && store.trialProduct == nil {
                        ProgressView()
                    }
                    Button("恢复购买") { Task { await store.restore() } }
                        .font(.callout)
                    if let message = store.message {
                        Text(message).font(.footnote).foregroundStyle(.red)
                    }
                }
                .padding()
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("关闭") { dismiss() } }
            }
            .onChange(of: store.entitlement) { _, state in
                if state.canWrite { dismiss() }
            }
        }
    }

    @ViewBuilder
    private func statusText(_ state: Entitlement, clock: Clock) -> some View {
        switch state {
        case .unlocked: Text("已解锁，感谢支持。")
        case .trial:
            let days = state.daysLeft(clock: clock)
            Text("试用中，还剩 \(days) 天。")
        case .trialNotStarted: Text("先免费试用 14 天，或直接一次性解锁。")
        case .expired: Text("试用已结束。查看对话仍然免费；发送、新建和审批需要解锁。")
        }
    }
}
