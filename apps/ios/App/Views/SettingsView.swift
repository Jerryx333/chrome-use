import ChromeUseKit
import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var renaming: GatewayClient?
    @State private var newName = ""
    @State private var removing: GatewayClient?
    @State private var workerURL = ""
    @State private var workerError: String?
    @State private var paywall = false

    var body: some View {
        NavigationStack {
            Form {
                Section("已配对的电脑") {
                    if model.clients.isEmpty { Text("还没有配对的电脑").foregroundStyle(.secondary) }
                    ForEach(model.clients) { client in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(client.credential.name)
                            Text(client.credential.url.absoluteString).font(.caption.monospaced()).foregroundStyle(.secondary)
                            let when = client.credential.pairedAt.formatted(date: .abbreviated, time: .shortened)
                            Text("配对于 \(when)").font(.caption2).foregroundStyle(.secondary)
                            if client.credential.fullAccessAllowed {
                                Label("此电脑允许手机操作高权限对话", systemImage: "exclamationmark.shield")
                                    .font(.caption).foregroundStyle(.orange)
                            }
                            if let version = client.credential.gatewayVersion {
                                Text("chrome-use \(version)").font(.caption2).foregroundStyle(.secondary)
                            }
                        }
                        .swipeActions {
                            Button("移除", role: .destructive) { removing = client }
                            Button("重命名") { newName = client.credential.name; renaming = client }.tint(.blue)
                        }
                        .contextMenu {
                            Button("重命名", systemImage: "pencil") { newName = client.credential.name; renaming = client }
                            Button("移除", systemImage: "trash", role: .destructive) { removing = client }
                        }
                    }
                    Button("配对新电脑") { dismiss(); model.pendingLink = nil; model.showPairing = true }
                }
                Section("解锁") {
                    LabeledContent("状态", value: entitlementText)
                    Button("购买或试用") { paywall = true }
                    Button("恢复购买") { Task { await model.store.restore() } }
                }
                Section {
                    Toggle("面容 ID 锁", isOn: Binding(get: { model.lock.active }, set: { on in Task { await model.lock.setEnabled(on) } }))
                        .disabled(model.lock.required || (!model.lock.available && !model.lock.enabled))
                } footer: {
                    if model.lock.required {
                        Text("有电脑允许手机操作高权限对话，所以必须开启：启动时和离开超过 5 分钟后都要验证。")
                    } else {
                        Text("开启后，每次回到 App 都需要验证。")
                    }
                }
                Section {
                    LabeledContent("通知权限", value: model.push.authorized == true ? String(localized: "已允许") : model.push.authorized == false ? String(localized: "未允许") : String(localized: "未询问"))
                    if model.push.authorized != true {
                        Button("允许通知") { model.push.requestAuthorization() }
                    }
                    LabeledContent("推送注册", value: model.push.relayToken == nil ? String(localized: "未注册") : String(localized: "已注册"))
                    if let error = model.push.lastError { Text(error).font(.caption).foregroundStyle(.red) }
                } header: {
                    Text("通知")
                } footer: {
                    Text("推送内容在电脑上端到端加密，推送服务只转发看不懂的密文。")
                }
                Section {
                    TextField("推送服务地址", text: $workerURL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .onSubmit(applyWorker)
                    HStack {
                        Button("保存", action: applyWorker)
                        Spacer()
                        Button("恢复默认") { model.push.resetWorkerURL(); workerURL = model.push.workerURL.absoluteString }
                    }
                    if let workerError { Text(workerError).font(.caption).foregroundStyle(.red) }
                } header: {
                    Text("高级")
                } footer: {
                    Text("自建推送 Worker 时才需要修改。")
                }
                Section("关于") {
                    LabeledContent("版本", value: AppConfig.version)
                    Text("chrome-use 遥控让你在手机上操作电脑上的 agent 面板；agent 和登录的 Chrome 都留在电脑上。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
            .navigationTitle("设置")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("完成") { dismiss() } }
            }
            .onAppear { workerURL = model.push.workerURL.absoluteString }
            .sheet(isPresented: $paywall) { PaywallView() }
            .alert("重命名", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
                TextField("名称", text: $newName)
                Button("保存") { if let renaming { model.rename(renaming.id, to: newName) } }
                Button("取消", role: .cancel) {}
            }
            .confirmationDialog("移除这台电脑？", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { client in
                Button("移除", role: .destructive) { model.remove(client.id) }
            } message: { _ in
                Text("手机会删除它的密钥。要彻底撤销，请在电脑上运行 chrome-use mobile revoke。")
            }
        }
    }

    private var entitlementText: String {
        let store = model.store
        switch store.entitlement {
        case .unlocked: return String(localized: "已解锁")
        case .trial: return String(localized: "试用中（剩 \(store.entitlement.daysLeft(clock: store.clock)) 天）")
        case .trialNotStarted: return String(localized: "未开始试用")
        case .expired: return String(localized: "试用已结束")
        }
    }

    private func applyWorker() {
        guard let url = URL(string: workerURL.trimmingCharacters(in: .whitespaces)), url.scheme == "https" || url.scheme == "http", url.host != nil else {
            workerError = String(localized: "请输入完整的 https:// 地址。")
            return
        }
        workerError = nil
        model.push.workerURL = url
    }
}
