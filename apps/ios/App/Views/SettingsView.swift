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
                Section("Paired computers") {
                    if model.clients.isEmpty { Text("No paired computers yet").foregroundStyle(.secondary) }
                    ForEach(model.clients) { client in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(client.credential.name)
                            Text(client.credential.url.absoluteString).font(.caption.monospaced()).foregroundStyle(.secondary)
                            let when = client.credential.pairedAt.formatted(date: .abbreviated, time: .shortened)
                            Text("Paired \(when)").font(.caption2).foregroundStyle(.secondary)
                            if client.credential.fullAccessAllowed {
                                Label("This computer allows high-access conversations from the phone", systemImage: "exclamationmark.shield")
                                    .font(.caption).foregroundStyle(.orange)
                                if !model.lock.passcodeSet {
                                    Text(AppModel.noPasscodeMessage).font(.caption).foregroundStyle(.red)
                                }
                            }
                            if let version = client.credential.gatewayVersion {
                                Text("chrome-use \(version)").font(.caption2).foregroundStyle(.secondary)
                            }
                        }
                        .swipeActions {
                            Button("Remove", role: .destructive) { removing = client }
                            Button("Rename") { newName = client.credential.name; renaming = client }.tint(.blue)
                        }
                        .contextMenu {
                            Button("Rename", systemImage: "pencil") { newName = client.credential.name; renaming = client }
                            Button("Remove", systemImage: "trash", role: .destructive) { removing = client }
                        }
                    }
                    Button("Pair a new computer") { dismiss(); model.pendingLink = nil; model.showPairing = true }
                }
                Section("Unlock") {
                    LabeledContent("Status", value: entitlementText)
                    Button("Buy or try") { paywall = true }.accessibilityIdentifier("buyOrTry")
                    Button("Restore purchases") { Task { await model.store.restore() } }
                }
                Section {
                    Toggle("Face ID lock", isOn: Binding(get: { model.lock.active && model.lock.passcodeSet }, set: { on in Task { await model.lock.setEnabled(on) } }))
                        .disabled(model.lock.required || (!model.lock.available && !model.lock.enabled))
                } footer: {
                    if model.lock.requirementUnenforceable {
                        Text(AppModel.noPasscodeMessage)
                    } else if model.lock.required {
                        Text("A computer lets this phone drive high-access conversations, so the lock is required: at launch and after more than 5 minutes away.")
                    } else {
                        Text("When on, you authenticate every time you return to the app.")
                    }
                }
                Section {
                    LabeledContent("Permission", value: model.push.authorized == true ? String(localized: "Allowed") : model.push.authorized == false ? String(localized: "Not allowed") : String(localized: "Not asked yet"))
                    if model.push.authorized != true {
                        Button("Allow notifications") { model.push.requestAuthorization() }
                    }
                    LabeledContent("Push registration", value: model.push.relayToken == nil ? String(localized: "Not registered") : String(localized: "Registered"))
                    if let error = model.push.lastError { Text(error).font(.caption).foregroundStyle(.red) }
                } header: {
                    Text("Notifications")
                } footer: {
                    Text("Notifications are end-to-end encrypted on the computer; the push service only forwards ciphertext it cannot read.")
                }
                Section {
                    TextField("Push service URL", text: $workerURL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .onSubmit(applyWorker)
                    HStack {
                        Button("Save", action: applyWorker)
                        Spacer()
                        Button("Reset to default") { model.push.resetWorkerURL(); workerURL = model.push.workerURL.absoluteString }
                    }
                    if let workerError { Text(workerError).font(.caption).foregroundStyle(.red) }
                } header: {
                    Text("Advanced")
                } footer: {
                    Text("Only change this if you run your own push Worker.")
                }
                Section("About") {
                    LabeledContent("Version", value: AppConfig.version)
                    Text("chrome-use Remote drives the agent panel on your computer from your phone; the agent and your signed-in Chrome stay on the computer.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
            .navigationTitle("Settings")
            .accessibilityIdentifier("settingsForm")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .onAppear { workerURL = model.push.workerURL.absoluteString }
            .sheet(isPresented: $paywall) { PaywallView() }
            .alert("Rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
                TextField("Name", text: $newName)
                Button("Save") { if let renaming { model.rename(renaming.id, to: newName) } }
                Button("Cancel", role: .cancel) {}
            }
            .confirmationDialog("Remove this computer?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { client in
                Button("Remove", role: .destructive) { model.remove(client.id) }
            } message: { _ in
                Text("The phone deletes its key. To revoke it completely, run chrome-use mobile revoke on the computer.")
            }
        }
    }

    private var entitlementText: String {
        let store = model.store
        switch store.entitlement {
        case .unlocked: return String(localized: "Unlocked")
        case .trial: return String(localized: "Trial (\(store.entitlement.daysLeft(clock: store.clock)) days left)")
        case .trialNotStarted: return String(localized: "Trial not started")
        case .expired: return String(localized: "Trial ended")
        }
    }

    private func applyWorker() {
        guard let url = PushRegistrar.workerURL(workerURL) else {
            workerError = String(localized: "Enter a full https:// URL.")
            return
        }
        workerError = nil
        model.push.workerURL = url
    }
}
