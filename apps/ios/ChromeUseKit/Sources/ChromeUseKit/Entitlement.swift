import Foundation

public protocol Clock: Sendable {
    var now: Date { get }
}

public struct SystemClock: Clock {
    public init() {}
    public var now: Date { Date() }
}

public struct FixedClock: Clock {
    public var now: Date
    public init(_ now: Date) { self.now = now }
}

/// Unlock state: a one-time purchase, or a 14-day trial started by the USD 0
/// "trial" purchase. Reading is always allowed; sending, creating and
/// answering need `canWrite`.
public enum Entitlement: Equatable, Sendable {
    case unlocked
    case trial(endsAt: Date)
    case trialNotStarted
    case expired(endedAt: Date)

    public static let trialLength: TimeInterval = 14 * 24 * 60 * 60

    /// `trialStart` is the verified purchase date of the trial product.
    public static func evaluate(unlocked: Bool, trialStart: Date?, clock: Clock) -> Entitlement {
        if unlocked { return .unlocked }
        guard let trialStart else { return .trialNotStarted }
        let end = trialStart.addingTimeInterval(trialLength)
        return clock.now < end ? .trial(endsAt: end) : .expired(endedAt: end)
    }

    public var canWrite: Bool {
        switch self {
        case .unlocked, .trial: return true
        case .trialNotStarted, .expired: return false
        }
    }

    /// Whole days left in the trial, rounded up (0 when not in a trial).
    public func daysLeft(clock: Clock) -> Int {
        guard case .trial(let end) = self else { return 0 }
        return max(0, Int((end.timeIntervalSince(clock.now) / 86_400).rounded(.up)))
    }
}
