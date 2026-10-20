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
    @ObservationIgnored private let passcodeCheck: () -> Bool
    /// The phone has a passcode, so the lock can be enforced. Without one
    /// a required lock would lock the person out: the app never locks then,
    /// and elevated operations are refused on the phone instead.
    private(set) var passcodeSet: Bool

    init(defaults: UserDefaults = .standard, clock: Clock = SystemClock(),
         passcodeSet: (() -> Bool)? = nil,
         authenticate: ((String) async throws -> Bool)? = nil) {
        self.defaults = defaults
        self.clock = clock
        let check = passcodeSet ?? AppLock.devicePasscodeSet
        self.passcodeCheck = check
        let passcode = check()
        self.passcodeSet = passcode
        self.authenticate = authenticate ?? { reason in
            try await LAContext().evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
        }
        let enabled = defaults.bool(forKey: Self.key)
        self.enabled = enabled
        self.locked = enabled && passcode
    }

    /// `deviceOwnerAuthentication` is available exactly when a passcode is set.
    static func devicePasscodeSet() -> Bool {
        var error: NSError?
        if LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) { return true }
        return (error as? LAError)?.code != .passcodeNotSet
    }

    /// A computer requires the lock but this phone cannot enforce it.
    var requirementUnenforceable: Bool { required && !passcodeSet }

    var active: Bool { enabled || required }

    var available: Bool {
        LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil)
    }

    /// `atLaunch`: a required lock locks right away; later (a computer just
    /// started allowing high access) it applies from the next return.
    func setRequired(_ required: Bool, atLaunch: Bool) {
        self.required = required
        if required && atLaunch && passcodeSet { locked = true }
        // The requirement lifted (access back to limited): nothing to unlock.
        if !active { locked = false }
    }

    func background() {
        backgroundedAt = clock.now
        if enabled && passcodeSet { locked = true }
    }

    func foreground() {
        defer { backgroundedAt = nil }
        // The person may have set or removed the passcode meanwhile.
        passcodeSet = passcodeCheck()
        if !passcodeSet { locked = false; return }
        guard required, !locked, let since = backgroundedAt else { return }
        if clock.now.timeIntervalSince(since) > Self.requiredRelockAfter { locked = true }
    }

    func unlock() async {
        guard active, passcodeSet else { locked = false; return }
        do {
            if try await authenticate(String(localized: "Unlock chrome-use")) {
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
        if (try? await authenticate(String(localized: "Turn on Face ID lock"))) == true {
            enabled = true
        }
    }
}
