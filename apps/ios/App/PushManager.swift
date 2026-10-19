import ChromeUseKit
import Foundation
import Observation
import UIKit
import UserNotifications

/// APNs registration, the push Worker's relay token, and keeping it fresh.
/// Each paired computer receives the token through `pushRegister` on every
/// connection (GatewayClient does that).
@MainActor
@Observable
final class PushManager {
    private(set) var authorized: Bool?
    private(set) var lastError: String?
    private(set) var registration: RelayRegistration?
    @ObservationIgnored var onRelayToken: ((String?) -> Void)?
    @ObservationIgnored private var deviceToken: String?
    @ObservationIgnored private let defaults = UserDefaults.standard
    @ObservationIgnored private let clock: Clock = SystemClock()

    static let workerKey = "pushWorkerURL"
    private static let registrationKey = "relayRegistration"

    init() {
        if let data = defaults.data(forKey: Self.registrationKey) {
            registration = try? JSONDecoder().decode(RelayRegistration.self, from: data)
        }
    }

    var relayToken: String? { registration?.relayToken }

    var workerURL: URL {
        get {
            defaults.string(forKey: Self.workerKey).flatMap(URL.init(string:)) ?? AppConfig.defaultPushWorker
        }
        set {
            defaults.set(newValue.absoluteString, forKey: Self.workerKey)
            forgetRegistration()
        }
    }

    func resetWorkerURL() {
        defaults.removeObject(forKey: Self.workerKey)
        forgetRegistration()
    }

    @ObservationIgnored private var lastRejection: Date?

    /// A gateway reported that the Worker refused our relay token: register
    /// again now. At most once a minute, so a misbehaving Worker cannot
    /// make the phone loop.
    func tokenRejected(reason: String) {
        if let lastRejection, Date().timeIntervalSince(lastRejection) < 60 { return }
        lastRejection = Date()
        lastError = String(localized: "The push token was refused (\(reason)); registering again.")
        forgetRegistration()
    }

    /// A token from another Worker is useless; drop it, also on disk.
    private func forgetRegistration() {
        registration = nil
        defaults.removeObject(forKey: Self.registrationKey)
        refreshIfNeeded()
    }

    func requestAuthorization() {
        Task {
            let center = UNUserNotificationCenter.current()
            let granted = (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
            authorized = granted
            if granted { UIApplication.shared.registerForRemoteNotifications() }
        }
    }

    func checkAuthorization() {
        Task {
            let settings = await UNUserNotificationCenter.current().notificationSettings()
            authorized = settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional
            if authorized == true { UIApplication.shared.registerForRemoteNotifications() }
        }
    }

    func didRegister(deviceToken data: Data) {
        deviceToken = data.map { String(format: "%02x", $0) }.joined()
        refreshIfNeeded()
    }

    func didFail(_ error: Error) {
        lastError = error.localizedDescription
    }

    /// Register with the Worker when there is no token, the APNs token
    /// changed, or the relay token expires within a week (90-day lifetime).
    func refreshIfNeeded() {
        guard let deviceToken else { return }
        if let registration, !registration.needsRefresh(deviceToken: deviceToken, clock: clock) {
            onRelayToken?(registration.relayToken)
            return
        }
        let registrar = PushRegistrar(baseURL: workerURL)
        Task {
            do {
                #if APNS_SANDBOX
                let sandbox = true
                #else
                let sandbox = false
                #endif
                let fresh = try await registrar.register(deviceToken: deviceToken, sandbox: sandbox)
                registration = fresh
                defaults.set(try JSONEncoder().encode(fresh), forKey: Self.registrationKey)
                lastError = nil
                onRelayToken?(fresh.relayToken)
            } catch {
                lastError = error.localizedDescription
            }
        }
    }
}
