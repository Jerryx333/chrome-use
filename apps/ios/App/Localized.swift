import ChromeUseKit
import SwiftUI

extension CloseReason {
    /// What the user reads when a connection ends or is refused.
    var message: String {
        switch self {
        case .malformed:
            return String(localized: "电脑拒绝了一条数据，连接已重置，正在重连。")
        case .unauthorized:
            return String(localized: "电脑不认识这部手机：配对可能已被撤销、禁用或已过期。请重新配对。")
        case .revoked:
            return String(localized: "这部手机已在电脑上被撤销，请重新配对。")
        case .handshakeTimeout:
            return String(localized: "握手超时，正在重试。")
        case .wrongProfile:
            return String(localized: "这部手机配对的是另一个浏览器配置，当前运行的网关不服务它。请在电脑上用对应的 --profile 启动 mobile serve。")
        case .backoff:
            return String(localized: "失败次数过多，电脑要求稍后再试。")
        case .gatewayNotAuthentic:
            return String(localized: "对方无法证明自己是已配对的电脑，已断开。请检查地址是否正确。")
        case .closed(let code):
            return code == nil
                ? String(localized: "无法连接到电脑。请确认 chrome-use mobile serve 正在运行，且手机能访问该地址。")
                : String(localized: "连接已断开，正在重连。")
        }
    }
}

extension GatewayClient.Status {
    var label: String {
        switch self {
        case .idle: return String(localized: "未连接")
        case .connecting: return String(localized: "正在连接…")
        case .connected: return String(localized: "已连接")
        case .retrying(_, let reason): return reason.message
        case .failed(let reason): return reason.message
        }
    }

    var color: Color {
        switch self {
        case .connected: return .green
        case .connecting, .retrying: return .orange
        case .idle: return .secondary
        case .failed: return .red
        }
    }
}

extension Phase {
    var label: String {
        switch self {
        case .idle: return String(localized: "空闲")
        case .starting: return String(localized: "启动中")
        case .running: return String(localized: "运行中")
        case .waitingInput: return String(localized: "等你处理")
        case .stopping: return String(localized: "停止中")
        case .failed: return String(localized: "失败")
        case .interrupted: return String(localized: "已中断")
        case .recoveryRequired: return String(localized: "需在电脑上确认")
        case .unknown: return String(localized: "未知")
        }
    }

    var tint: Color {
        switch self {
        case .waitingInput: return .orange
        case .starting, .running, .stopping: return .blue
        case .failed, .recoveryRequired: return .red
        case .interrupted: return .gray
        default: return .secondary
        }
    }
}

extension ToolAccess {
    var label: String {
        switch self {
        case .browserOnly: return String(localized: "仅浏览器")
        case .readFiles: return String(localized: "可读文件")
        case .writeFiles: return String(localized: "可写文件")
        case .runCommands: return String(localized: "可运行命令")
        }
    }
}

extension DecisionOption {
    var label: String {
        switch kind {
        case .allowOnce: return String(localized: "允许一次")
        case .allowForSession: return String(localized: "本会话都允许")
        case .allowTurn: return String(localized: "本轮允许")
        case .deny: return String(localized: "拒绝")
        case .decline: return String(localized: "拒绝")
        case .cancel: return String(localized: "取消请求")
        case .agentOption(let name, _): return name
        }
    }
}

enum ToolLabel {
    static func text(_ name: String) -> String {
        if name.hasPrefix("browser:") { return String(localized: "浏览器：\(String(name.dropFirst("browser:".count)))") }
        switch name {
        case "command": return String(localized: "运行命令")
        case "fileChange": return String(localized: "修改文件")
        case "webSearch": return String(localized: "搜索网页")
        case "toolResult": return String(localized: "工具结果")
        default: return name
        }
    }
}

extension GatewayError {
    var message: String {
        switch self {
        case .closed(let reason): return reason.message
        case .remote(let code, let message):
            switch code {
            case "denied": return String(localized: "手机端不允许这个操作：\(message)")
            case "rateLimited": return String(localized: "操作太频繁，请稍后再试。")
            case "screenshot": return String(localized: "无法截图：这个对话的浏览器会话没有在运行。")
            default: return message.isEmpty ? code : message
            }
        case .timeout: return String(localized: "电脑没有及时回应。")
        case .notConnected: return String(localized: "还没连上电脑。")
        case .protocolViolation(let what): return String(localized: "通信出错：\(what)")
        }
    }
}

func describe(_ error: Error) -> String {
    if let error = error as? GatewayError { return error.message }
    if let error = error as? PairLink.ParseError {
        switch error {
        case .notAPairLink: return String(localized: "这不是 chrome-use 配对链接。")
        case .unsupportedVersion: return String(localized: "配对链接版本不受支持，请更新 App。")
        case .plaintextToPublicHost: return String(localized: "公网地址必须用 wss://。ws:// 只能用于 Tailscale 或局域网地址。")
        default: return String(localized: "配对链接不完整或已损坏。")
        }
    }
    if let error = error as? ImagePreparer.Failure {
        return error == .tooLarge ? String(localized: "图片太大，压缩后仍超过 512 KiB。") : String(localized: "无法读取这张图片。")
    }
    return error.localizedDescription
}
