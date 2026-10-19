import ChromeUseKit
import Foundation
import LocalAuthentication
import Observation

/// Face ID / passcode lock. Optional (asked again after every return from
/// the background), and required when a paired computer lets this phone
/// drive high-access conversations: then on launch and after more than
/// five minutes in the background.
@MainActor
@Observable
final class AppLock {
    private static let key = "biometricLock"
    static let requiredRelockAfter: TimeInterval = 5 * 60

    /// The user's own setting.
    var enabled: Bool {
        didSet { defaults.set(enabled, forKey: Self.key) }
    }
    /// Forced by a computer's `fullAccessAllowed`.
    private(set) var required = false
    private(set) var locked: Bool
    private(set) var error: String?
    @ObservationIgnored private var backgroundedAt: Date?
    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private let clock: Clock
    @ObservationIgnored private let authenticate: (String) async throws -> Bool

    init(defaults: UserDefaults = .standard, clock: Clock = SystemClock(),
         authenticate: ((String) async throws -> Bool)? = nil) {
        self.defaults = defaults
        self.clock = clock
        self.authenticate = authenticate ?? { reason in
            try await LAContext().evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
        }
        let enabled = defaults.bool(forKey: Self.key)
        self.enabled = enabled
        self.locked = enabled
    }

    var active: Bool { enabled || required }

    var available: Bool {
        LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil)
    }

    /// `atLaunch`: a required lock locks right away; later (a computer just
    /// started allowing high access) it applies from the next return.
    func setRequired(_ required: Bool, atLaunch: Bool) {
        self.required = required
        if required && atLaunch { locked = true }
    }

    func background() {
        backgroundedAt = clock.now
        if enabled { locked = true }
    }

    func foreground() {
        defer { backgroundedAt = nil }
        guard required, !locked, let since = backgroundedAt else { return }
        if clock.now.timeIntervalSince(since) > Self.requiredRelockAfter { locked = true }
    }

    func unlock() async {
        guard active else { locked = false; return }
        do {
            if try await authenticate(String(localized: "解锁 chrome-use")) {
                locked = false
                error = nil
            }
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// Turning the lock on is confirmed with the same authentication; it
    /// cannot be turned off while a computer requires it.
    func setEnabled(_ on: Bool) async {
        guard on else { enabled = false; if !required { locked = false }; return }
        if (try? await authenticate(String(localized: "开启面容 ID 锁"))) == true {
            enabled = true
        }
    }
}
