import Foundation

/// Where a plain `ws://` gateway is acceptable: the user's own tailnet or
/// LAN. Frames are end-to-end encrypted either way, but a public host gets
/// `wss://` only (and App Transport Security allows nothing else there).
public enum TransportPolicy {
    public static func allowsPlaintext(host rawHost: String) -> Bool {
        let host = rawHost.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]."))
        if host == "localhost" || host.hasSuffix(".local") || host.hasSuffix(".ts.net") || host == "ts.net" { return true }
        if let v4 = ipv4(host) {
            let (a, b) = (v4[0], v4[1])
            return a == 10 || a == 127
                || (a == 172 && (16...31).contains(b))
                || (a == 192 && b == 168)
                || (a == 169 && b == 254)
                || (a == 100 && (64...127).contains(b))
        }
        if host.contains(":") {
            // IPv6: loopback, unique local (fc00::/7), link local (fe80::/10),
            // Tailscale's fd7a:115c:a1e0::/48 falls in fc00::/7.
            if host == "::1" { return true }
            guard let first = host.split(separator: ":", omittingEmptySubsequences: false).first,
                  let word = UInt16(first, radix: 16) else { return false }
            return word & 0xFE00 == 0xFC00 || word & 0xFFC0 == 0xFE80
        }
        // An unqualified name (no dot) only resolves on the local network.
        return !host.contains(".")
    }

    static func ipv4(_ host: String) -> [Int]? {
        let parts = host.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return nil }
        let numbers = parts.compactMap { Int($0) }.filter { (0...255).contains($0) }
        return numbers.count == 4 ? numbers : nil
    }
}
