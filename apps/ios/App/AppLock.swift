import Foundation
import LocalAuthentication
import Observation

/// Optional Face ID / passcode lock, asked again whenever the app returns
/// from the background.
@MainActor
@Observable
final class AppLock {
    private static let key = "biometricLock"
    var enabled: Bool {
        didSet { UserDefaults.standard.set(enabled, forKey: Self.key) }
    }
    private(set) var locked: Bool
    private(set) var error: String?

    init() {
        let enabled = UserDefaults.standard.bool(forKey: Self.key)
        self.enabled = enabled
        self.locked = enabled
    }

    var available: Bool {
        LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil)
    }

    func background() {
        if enabled { locked = true }
    }

    func unlock() async {
        guard enabled else { locked = false; return }
        let context = LAContext()
        do {
            if try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: String(localized: "解锁 chrome-use")) {
                locked = false
                error = nil
            }
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// Turning the lock on is confirmed with the same authentication.
    func setEnabled(_ on: Bool) async {
        guard on else { enabled = false; locked = false; return }
        let context = LAContext()
        if (try? await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: String(localized: "开启面容 ID 锁"))) == true {
            enabled = true
        }
    }
}
