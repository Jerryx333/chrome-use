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
        let credentials = (try? KeychainCredentialStore(accessGroup: group).all()) ?? []
        if let payload = request.content.userInfo["e"] as? String,
           let notice = PushNotice.open(payload: payload, credentials: credentials) {
            content.title = notice.title.isEmpty ? "chrome-use" : notice.title
            content.body = body(for: notice)
            content.threadIdentifier = notice.conversationId.isEmpty ? notice.deviceId : notice.conversationId
            var info = content.userInfo
            for (key, value) in notice.userInfo { info[key] = value }
            content.userInfo = info
        } else {
            generic(content)
        }
        contentHandler(content)
    }

    /// Fixed bodies are said in the user's language (by the payload's
    /// `code`); summaries and agent text are shown as sent.
    private func body(for notice: PushNotice) -> String {
        switch notice.standardBody {
        case .waitingForApproval: return String(localized: "等你审批")
        case .finished: return String(localized: "已完成")
        case .stopped: return String(localized: "已停止")
        case .failed: return String(localized: "失败")
        case .test: return String(localized: "测试通知")
        case nil: return notice.body
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
