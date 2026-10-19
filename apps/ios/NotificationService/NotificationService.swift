import ChromeUseKit
import UserNotifications

/// Decrypts the gateway's end-to-end encrypted push (`e`) with the push key
/// of whichever paired computer sent it, and rewrites title and body. When
/// that fails the generic alert stays, in the user's language.
final class NotificationService: UNNotificationServiceExtension {
    private var handler: ((UNNotificationContent) -> Void)?
    private var content: UNMutableNotificationContent?

    override func didReceive(_ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        handler = contentHandler
        guard let content = request.content.mutableCopy() as? UNMutableNotificationContent else {
            contentHandler(request.content)
            return
        }
        self.content = content
        let group = Bundle.main.object(forInfoDictionaryKey: "ChromeUseKeychainGroup") as? String
        // Push keys only; the full pairing secrets stay in the app's own group.
        let keys = (try? KeychainPushKeyStore(accessGroup: group).all()) ?? []
        if let payload = request.content.userInfo["e"] as? String,
           let notice = PushNotice.open(payload: payload, keys: keys) {
            content.title = notice.title.isEmpty ? "chrome-use" : notice.title
            // Worded by the code, never by the English body; an agent's own
            // text (turn preview, schedule result) goes under the reason.
            let reason = reason(for: notice.code)
            if notice.showsBody {
                content.subtitle = reason
                content.body = notice.body
            } else {
                content.body = reason
            }
            content.threadIdentifier = notice.conversationId.isEmpty ? notice.deviceId : notice.conversationId
            var info = content.userInfo
            for (key, value) in notice.userInfo { info[key] = value }
            content.userInfo = info
        } else {
            generic(content)
        }
        contentHandler(content)
    }

    private func reason(for code: PushNotice.Code?) -> String {
        switch code {
        case .needsInput: return String(localized: "等你审批")
        case .finished: return String(localized: "已完成")
        case .stopped: return String(localized: "已停止")
        case .failed: return String(localized: "失败")
        case .scheduleDone: return String(localized: "定时任务已完成")
        case .scheduleFailed: return String(localized: "定时任务失败")
        case .test: return String(localized: "测试通知")
        case nil: return String(localized: "chrome-use 需要你")
        }
    }

    private func generic(_ content: UNMutableNotificationContent) {
        content.title = "chrome-use"
        content.body = String(localized: "chrome-use 需要你")
    }

    override func serviceExtensionTimeWillExpire() {
        if let handler, let content {
            generic(content)
            handler(content)
        }
    }
}
