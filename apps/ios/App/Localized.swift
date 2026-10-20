import ChromeUseKit
import SwiftUI

extension CloseReason {
    /// What the user reads when a connection ends or is refused.
    var message: String {
        switch self {
        case .malformed:
            return String(localized: "The computer rejected a message; the connection was reset and is reconnecting.")
        case .unauthorized:
            return String(localized: "The computer does not recognize this phone: the pairing may have been revoked, disabled or expired. Pair again.")
        case .revoked:
            return String(localized: "This phone was revoked on the computer. Pair again.")
        case .handshakeTimeout:
            return String(localized: "Handshake timed out; retrying.")
        case .wrongProfile:
            return String(localized: "This phone is paired to another browser profile than the running gateway serves. Start mobile serve with the matching --profile on the computer.")
        case .backoff:
            return String(localized: "Too many failed attempts; the computer asks to try again later.")
        case .gatewayNotAuthentic:
            return String(localized: "The other side could not prove it is the paired computer, so the connection was closed. Check the address.")
        case .closed(let code):
            return code == nil
                ? String(localized: "Cannot reach the computer. Make sure chrome-use mobile serve is running and the phone can reach its address.")
                : String(localized: "Disconnected; reconnecting.")
        }
    }
}

extension GatewayClient.Status {
    var label: String {
        switch self {
        case .idle: return String(localized: "Not connected")
        case .connecting: return String(localized: "Connecting…")
        case .connected: return String(localized: "Connected")
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
        case .idle: return String(localized: "Idle")
        case .starting: return String(localized: "Starting")
        case .running: return String(localized: "Running")
        case .waitingInput: return String(localized: "Needs you")
        case .stopping: return String(localized: "Stopping")
        case .failed: return String(localized: "Failed")
        case .interrupted: return String(localized: "Interrupted")
        case .recoveryRequired: return String(localized: "Confirm on computer")
        case .unknown: return String(localized: "Unknown")
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
        case .browserOnly: return String(localized: "Browser only")
        case .readFiles: return String(localized: "Read files")
        case .writeFiles: return String(localized: "Write files")
        case .runCommands: return String(localized: "Run commands")
        }
    }
}

extension DecisionOption {
    var label: String {
        switch kind {
        case .allowOnce: return String(localized: "Allow once")
        case .allowForSession: return String(localized: "Allow for this session")
        case .allowTurn: return String(localized: "Allow for this turn")
        case .deny: return String(localized: "Deny")
        case .decline: return String(localized: "Deny")
        case .cancel: return String(localized: "Cancel request")
        case .agentOption(let name, _): return name
        }
    }
}

enum ToolLabel {
    static func text(_ name: String) -> String {
        if name.hasPrefix("browser:") {
            let tool = String(name.dropFirst("browser:".count))
            return String(localized: "Browser: \(tool)")
        }
        switch name {
        case "command": return String(localized: "Ran a command")
        case "fileChange": return String(localized: "Changed files")
        case "webSearch": return String(localized: "Searched the web")
        case "toolResult": return String(localized: "Tool result")
        default: return name
        }
    }
}

extension GatewayError {
    var message: String { message(deviceId: nil) }

    /// `deviceId` fills in the command for `accessTooHigh`.
    func message(deviceId: String?) -> String {
        switch self {
        case .closed(let reason): return reason.message
        case .remote(let code, let message):
            switch code {
            case "denied": return String(localized: "Not allowed from the phone: \(message)")
            case "rateLimited": return String(localized: "Too many requests; try again shortly.")
            case "screenshot": return String(localized: "Cannot take a screenshot: this conversation’s browser session is not running.")
            case "accessTooHigh":
                let id = deviceId ?? "<id>"
                return String(localized: "This conversation’s access is too high. Run `chrome-use mobile access \(id) full` on the computer to drive it from the phone")
            default: return message.isEmpty ? code : message
            }
        case .timeout: return String(localized: "The computer did not answer in time.")
        case .notConnected: return String(localized: "Not connected to the computer yet.")
        case .protocolViolation(let what): return String(localized: "Communication error: \(what)")
        }
    }
}

func describe(_ error: Error, deviceId: String? = nil) -> String {
    if let error = error as? GatewayError { return error.message(deviceId: deviceId) }
    if let error = error as? PairLink.ParseError {
        switch error {
        case .notAPairLink: return String(localized: "This is not a chrome-use pairing link.")
        case .unsupportedVersion: return String(localized: "This pairing link version is not supported; update the app.")
        case .plaintextToPublicHost: return String(localized: "Public addresses need wss://. ws:// is only for Tailscale or local network addresses.")
        default: return String(localized: "The pairing link is incomplete or damaged.")
        }
    }
    if let error = error as? ImagePreparer.Failure {
        return error == .tooLarge ? String(localized: "The image is still larger than 512 KiB after compression.") : String(localized: "Cannot read this image.")
    }
    return error.localizedDescription
}
