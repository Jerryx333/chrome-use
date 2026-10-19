import Foundation
import Network

/// iOS asks for Local Network permission on the first connection to a LAN
/// address, and that connection fails while the prompt is up. Pairing
/// touches the address as soon as the sheet opens, and retries a plain
/// network failure while the person answers the prompt.
public enum LocalNetwork {
    /// Hosts the Local Network permission covers: private and link-local
    /// addresses, `.local` and unqualified names. Tailscale (100.64/10,
    /// *.ts.net) runs over a VPN interface and needs no prompt.
    public static func needsPermission(host rawHost: String) -> Bool {
        let host = rawHost.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]."))
        if host.hasSuffix(".ts.net") { return false }
        if let v4 = TransportPolicy.ipv4(host) {
            let (a, b) = (v4[0], v4[1])
            return a == 10 || (a == 172 && (16...31).contains(b)) || (a == 192 && b == 168) || (a == 169 && b == 254)
        }
        if host.contains(":") {
            guard let first = host.split(separator: ":", omittingEmptySubsequences: false).first,
                  let word = UInt16(first, radix: 16) else { return false }
            // Unique local outside Tailscale's fd7a:115c:a1e0::/48, and link local.
            return (word & 0xFE00 == 0xFC00 && !host.hasPrefix("fd7a:115c:a1e0")) || word & 0xFFC0 == 0xFE80
        }
        return host.hasSuffix(".local") || (!host.contains(".") && host != "localhost")
    }

    /// Open and drop a TCP connection to the gateway's address, which makes
    /// iOS show the permission prompt now rather than mid-pairing.
    public static func touch(_ url: URL) {
        guard let host = url.host, needsPermission(host: host) else { return }
        let port = NWEndpoint.Port(rawValue: UInt16(url.port ?? (url.scheme == "wss" ? 443 : 80))) ?? .http
        let connection = NWConnection(host: NWEndpoint.Host(host), port: port, using: .tcp)
        connection.stateUpdateHandler = { state in
            switch state {
            case .ready, .failed, .cancelled: connection.cancel()
            default: break
            }
        }
        connection.start(queue: .global(qos: .utility))
        DispatchQueue.global().asyncAfter(deadline: .now() + 15) { connection.cancel() }
    }

    /// Delay before pairing attempt `attempt` (1-based) is retried, or nil.
    /// Only a plain network failure before `pairComplete` left the phone is
    /// retried: about 30 s in all, while the person answers the prompt.
    public static func pairingRetryDelay(after error: Error, attempt: Int, host: String) -> TimeInterval? {
        guard needsPermission(host: host), attempt <= 3,
              let gateway = error as? GatewayError, gateway == .closed(.closed(code: nil))
        else { return nil }
        return [5, 10, 15][attempt - 1]
    }
}
