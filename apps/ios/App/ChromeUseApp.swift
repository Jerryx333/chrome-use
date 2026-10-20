import ChromeUseKit
import SwiftUI
import UIKit
import UserNotifications

final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    var model: AppModel?

    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Task { @MainActor in model?.push.didRegister(deviceToken: deviceToken) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Task { @MainActor in model?.push.didFail(error) }
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner, .sound, .list]
    }

    /// Tapping a notification opens its conversation; for `needsInput` the
    /// approval card is at the bottom of that view.
    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let info = response.notification.request.content.userInfo
        await MainActor.run { model?.openNotification(userInfo: info) }
    }
}

@main
struct ChromeUseApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) var delegate
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .onAppear {
                    delegate.model = model
                    model.push.checkAuthorization()
                }
                .onOpenURL { model.open(url: $0) }
                .onChange(of: scenePhase) { _, phase in
                    switch phase {
                    case .active: model.foreground()
                    case .background: model.lock.background()
                    default: break
                    }
                }
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        let locked = model.lock.locked
        NavigationStack(path: $model.path) {
            ComputersView()
                .navigationDestination(for: Route.self) { route in
                    switch route {
                    case .computer(let id):
                        if let client = model.client(id) { ConversationsView(client: client) }
                    case .conversation(let id, let conversationId):
                        if let client = model.client(id) { ConversationView(client: client, conversationId: conversationId) }
                    case .schedules(let id):
                        if let client = model.client(id) { SchedulesView(client: client) }
                    }
                }
        }
        // While locked nothing behind the lock is drawn or readable by
        // accessibility; state (navigation, drafts) is kept.
        .opacity(locked ? 0 : 1)
        .accessibilityHidden(locked)
        .allowsHitTesting(!locked)
        .sheet(isPresented: $model.showPairing) { PairingView() }
        .sheet(isPresented: $model.showSettings) { SettingsView() }
        .sheet(isPresented: $model.showPaywall) { PaywallView() }
        .overlay {
            if locked { LockView() }
        }
        .onChange(of: locked) { _, now in
            if now { model.showPairing = false; model.showSettings = false; model.showPaywall = false }
        }
    }
}

struct LockView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ZStack {
            Rectangle().fill(.regularMaterial).ignoresSafeArea()
            VStack(spacing: 16) {
                Image(systemName: "lock.fill").font(.system(size: 44))
                Text("chrome-use is locked").font(.title3.bold())
                if let error = model.lock.error { Text(error).font(.footnote).foregroundStyle(.secondary) }
                Button("Unlock") { Task { await model.lock.unlock() } }
                    .buttonStyle(.borderedProminent)
            }
        }
        .task { await model.lock.unlock() }
    }
}
