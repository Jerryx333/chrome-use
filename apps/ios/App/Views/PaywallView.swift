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
                    Text("Unlock chrome-use Remote").font(.title2.bold()).accessibilityIdentifier("paywallTitle")
                    statusText(store.entitlement, clock: store.clock)
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 10) {
                        Label("Send messages and start conversations from your phone", systemImage: "paperplane")
                        Label("Answer the agent’s approval requests anywhere", systemImage: "hand.raised")
                        Label("Stop turns, take screenshots, run schedules", systemImage: "camera.viewfinder")
                        Label("Pay once, keep it forever; reading is always free", systemImage: "checkmark.seal")
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
                    .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 14))
                    let trialFirst = store.entitlement == .trialNotStarted
                    if let trial = store.trialProduct, trialFirst {
                        Button {
                            Task { await store.buy(trial) }
                        } label: {
                            Text("Start 14-day free trial").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.borderedProminent)
                        .controlSize(.large)
                        .disabled(store.purchasing)
                    }
                    if let unlock = store.unlockProduct, store.entitlement != .unlocked {
                        let button = Button {
                            Task { await store.buy(unlock) }
                        } label: {
                            Text("Unlock for \(unlock.displayPrice)").frame(maxWidth: .infinity)
                        }
                        .controlSize(.large)
                        .disabled(store.purchasing)
                        // Before the trial, starting it is the primary action.
                        if trialFirst { button.buttonStyle(.bordered) } else { button.buttonStyle(.borderedProminent) }
                    }
                    if store.unlockProduct == nil && store.trialProduct == nil {
                        ProgressView()
                    }
                    Button("Restore purchases") { Task { await store.restore() } }
                        .font(.callout)
                    if let message = store.message {
                        Text(message).font(.footnote).foregroundStyle(.red)
                    }
                }
                .padding()
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } }
            }
            .onChange(of: store.entitlement) { _, state in
                if state.canWrite { dismiss() }
            }
        }
    }

    @ViewBuilder
    private func statusText(_ state: Entitlement, clock: Clock) -> some View {
        switch state {
        case .unlocked: Text("Unlocked. Thanks for your support.")
        case .trial:
            let days = state.daysLeft(clock: clock)
            Text("Trial: \(days) days left.")
        case .trialNotStarted: Text("Try it free for 14 days, or unlock once.")
        case .expired: Text("Your trial has ended. Reading, stopping and declining stay free; sending, starting conversations and allowing requests need the unlock.")
        }
    }
}

