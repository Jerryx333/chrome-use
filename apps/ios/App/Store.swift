import ChromeUseKit
import Foundation
import Observation
import StoreKit

/// StoreKit 2 purchases, verified on the device. The unlock is a
/// non-consumable; the trial is a USD 0 non-consumable whose purchase date
/// starts the 14 days.
@MainActor
@Observable
final class Store {
    private(set) var unlockProduct: Product?
    private(set) var trialProduct: Product?
    private(set) var unlocked = false
    private(set) var trialStart: Date?
    private(set) var purchasing = false
    var message: String?
    @ObservationIgnored let clock: Clock
    @ObservationIgnored private var updates: Task<Void, Never>?

    #if DEBUG
    private static let launched = Date()
    #endif

    init(clock: Clock = SystemClock()) {
        self.clock = clock
        updates = Task { [weak self] in
            for await update in Transaction.updates {
                if case .verified(let transaction) = update { await transaction.finish() }
                await self?.refresh()
            }
        }
        Task { await load() }
    }

    var entitlement: Entitlement {
        #if DEBUG
        // UI tests (screenshots) run with a trial that started now.
        if ProcessInfo.processInfo.environment["CHROMEUSE_UITEST_TRIAL"] == "1", trialStart == nil, !unlocked {
            return Entitlement.evaluate(unlocked: false, trialStart: Self.launched, clock: clock)
        }
        #endif
        return Entitlement.evaluate(unlocked: unlocked, trialStart: trialStart, clock: clock)
    }

    func load() async {
        do {
            let products = try await Product.products(for: [AppConfig.unlockProduct, AppConfig.trialProduct])
            unlockProduct = products.first { $0.id == AppConfig.unlockProduct }
            trialProduct = products.first { $0.id == AppConfig.trialProduct }
        } catch {
            message = String(localized: "无法载入商品：\(error.localizedDescription)")
        }
        await refresh()
    }

    /// Only transactions StoreKit verified count; revoked ones do not.
    func refresh() async {
        var unlocked = false
        var trialStart: Date?
        for await result in Transaction.currentEntitlements {
            guard case .verified(let transaction) = result, transaction.revocationDate == nil else { continue }
            if transaction.productID == AppConfig.unlockProduct { unlocked = true }
            if transaction.productID == AppConfig.trialProduct { trialStart = transaction.originalPurchaseDate }
        }
        self.unlocked = unlocked
        self.trialStart = trialStart
    }

    func buy(_ product: Product) async {
        purchasing = true
        defer { purchasing = false }
        do {
            switch try await product.purchase() {
            case .success(.verified(let transaction)):
                await transaction.finish()
            case .success(.unverified(_, let error)):
                message = String(localized: "购买未通过验证：\(error.localizedDescription)")
            case .pending:
                message = String(localized: "购买等待批准。")
            case .userCancelled:
                break
            @unknown default:
                break
            }
        } catch {
            message = error.localizedDescription
        }
        await refresh()
    }

    func restore() async {
        do {
            try await AppStore.sync()
        } catch {
            message = error.localizedDescription
        }
        await refresh()
    }
}
