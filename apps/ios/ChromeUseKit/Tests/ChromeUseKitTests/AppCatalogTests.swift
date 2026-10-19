import Foundation
import XCTest
@testable import ChromeUseKit

/// The app's String Catalogs and App Store texts, read from the source tree
/// (apps/ios), so `swift test` fails on any missing translation or on a UI
/// string the catalogs do not know.
final class AppCatalogTests: XCTestCase {
    static let locales = ["en", "zh-Hans", "zh-Hant", "ja", "ko", "es", "fr", "de", "pt-BR"]
    static let ios = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

    func catalog(_ path: String) throws -> [String: JSONValue] {
        let value = try JSONValue.decode(Data(contentsOf: Self.ios.appendingPathComponent(path)))
        XCTAssertEqual(value["sourceLanguage"], "en", path)
        return try XCTUnwrap(value["strings"].object, path)
    }

    static func placeholders(_ text: String) -> [String] {
        let regex = try! NSRegularExpression(pattern: "%(?:@|lld)")
        return regex.matches(in: text, range: NSRange(text.startIndex..., in: text)).map { String(text[Range($0.range, in: text)!]) }
    }

    static func backticked(_ text: String) -> [String] {
        text.components(separatedBy: "`").enumerated().filter { $0.offset % 2 == 1 }.map(\.element)
    }

    func testEveryStringIsTranslatedIntoEveryLanguage() throws {
        for path in ["App/Localizable.xcstrings", "NotificationService/Localizable.xcstrings",
                     "App/InfoPlist.xcstrings", "NotificationService/InfoPlist.xcstrings"] {
            let strings = try catalog(path)
            XCTAssertFalse(strings.isEmpty, path)
            let infoPlist = path.hasSuffix("InfoPlist.xcstrings")
            for (key, entry) in strings {
                let english = infoPlist ? entry["localizations"]["en"]["stringUnit"]["value"].string ?? "" : key
                for locale in Self.locales {
                    let unit = entry["localizations"][locale]["stringUnit"]
                    let value = unit["value"].string ?? ""
                    XCTAssertEqual(unit["state"], "translated", "\(path) \(locale): \(key)")
                    XCTAssertFalse(value.trimmingCharacters(in: .whitespaces).isEmpty, "\(path) \(locale): \(key)")
                    XCTAssertEqual(Self.placeholders(value), Self.placeholders(english), "\(path) \(locale) placeholders: \(key)")
                    XCTAssertEqual(Self.backticked(value), Self.backticked(english), "\(path) \(locale) keeps commands: \(key)")
                    if english.contains("chrome-use") {
                        XCTAssertTrue(value.contains("chrome-use"), "\(path) \(locale) keeps the product name: \(key)")
                    }
                }
            }
        }
    }

    /// Every user-facing literal in the app and the extension is a catalog
    /// key, and no Chinese (or other non-source) text is hard-coded.
    func testNoHardCodedStrings() throws {
        let initializers = #"(?:Text|Button|Label|LabeledContent|Section|Toggle|TextField|Picker|ContentUnavailableView|navigationTitle|confirmationDialog|alert|accessibilityLabel|Menu|LocalizedStringKey|String\(localized:)\(\s*"((?:[^"\\]|\\.)*)""#
        let literal = try NSRegularExpression(pattern: initializers)
        let ternary = try NSRegularExpression(pattern: #"(?:Text|Button|Label|navigationTitle|accessibilityLabel)\(\s*[A-Za-z_.!=<> ]*\?\s*""#)
        let cjk = try NSRegularExpression(pattern: "[\\u3040-\\u30ff\\u3400-\\u9fff\\uac00-\\ud7af]")
        let appKeys = Set(try catalog("App/Localizable.xcstrings").keys)
        let extensionKeys = Set(try catalog("NotificationService/Localizable.xcstrings").keys)
        let normalize = { (key: String) in key.replacingOccurrences(of: "%lld", with: "%@") }
        let known = (appKeys: Set(appKeys.map(normalize)), extensionKeys: Set(extensionKeys.map(normalize)))
        let files = try FileManager.default.subpathsOfDirectory(atPath: Self.ios.path)
            .filter { ($0.hasPrefix("App/") || $0.hasPrefix("NotificationService/")) && $0.hasSuffix(".swift") }
        XCTAssertFalse(files.isEmpty)
        var checked = 0
        for file in files {
            let source = try String(contentsOf: Self.ios.appendingPathComponent(file), encoding: .utf8)
            let range = NSRange(source.startIndex..., in: source)
            let code = source.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }.joined(separator: "\n")
            XCTAssertNil(cjk.firstMatch(in: code, range: NSRange(code.startIndex..., in: code)), "\(file) has hard-coded CJK text")
            XCTAssertNil(ternary.firstMatch(in: source, range: range), "\(file): a ternary of plain strings is not localized; use LocalizedStringKey")
            let keys = file.hasPrefix("App/") ? known.appKeys : known.extensionKeys
            for match in literal.matches(in: source, range: range) {
                var text = String(source[Range(match.range(at: 1), in: source)!])
                text = text.replacingOccurrences(of: #"\\\((?:[^()]|\([^()]*\))*\)"#, with: "%@", options: .regularExpression)
                text = text.replacingOccurrences(of: #"\""#, with: "\"")
                // Symbol names, identifiers and pure punctuation are not UI text.
                guard text.rangeOfCharacter(from: .letters) != nil, !text.hasPrefix("%@") || text.count > 2 else { continue }
                if ["chrome-use", "Agent"].contains(text) && keys.contains(text) { checked += 1; continue }
                XCTAssertTrue(keys.contains(normalize(text)), "\(file): \"\(text)\" is not in the catalog")
                checked += 1
            }
        }
        XCTAssertGreaterThan(checked, 120, "the scan found the UI strings")
    }

    func testAppStoreListings() throws {
        func section(_ text: String, _ name: String) -> String {
            guard let start = text.range(of: "## \(name)\n") else { return "" }
            let rest = text[start.upperBound...]
            let end = rest.range(of: "\n## ")?.lowerBound ?? rest.endIndex
            return rest[..<end].trimmingCharacters(in: .whitespacesAndNewlines)
        }
        for locale in Self.locales {
            let text = try String(contentsOf: Self.ios.appendingPathComponent("docs/app-store/\(locale).md"), encoding: .utf8)
            XCTAssertEqual(section(text, "Name"), "chrome-use Remote", locale)
            let subtitle = section(text, "Subtitle")
            XCTAssertTrue((1...30).contains(subtitle.count), "\(locale) subtitle \(subtitle.count)")
            let keywords = section(text, "Keywords")
            XCTAssertTrue((1...100).contains(keywords.count), "\(locale) keywords \(keywords.count)")
            XCTAssertFalse(keywords.contains(", "), "\(locale) keywords: no spaces after commas")
            XCTAssertTrue((1...170).contains(section(text, "Promotional text").count), "\(locale) promotional text")
            XCTAssertTrue((1...4000).contains(section(text, "Description").count), "\(locale) description")
            XCTAssertTrue(section(text, "In-app purchases").contains("com.leeguoo.chromeuse.unlock"), locale)
        }
        // In-app purchase texts per locale, within App Store Connect's limits.
        let storekit = try JSONValue.decode(Data(contentsOf: Self.ios.appendingPathComponent("App/Products.storekit")))
        for product in storekit["products"].array ?? [] {
            let localizations = product["localizations"].array ?? []
            XCTAssertEqual(localizations.count, Self.locales.count, product["productID"].string ?? "")
            for localization in localizations {
                XCTAssertTrue((1...30).contains(localization["displayName"].string?.count ?? 0), "\(localization)")
                XCTAssertTrue((1...45).contains(localization["description"].string?.count ?? 0), "\(localization)")
            }
        }
    }
}
