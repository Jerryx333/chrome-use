import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// Images for `conversationAttachmentPut`: at most 5 per message, each a
/// JPEG of at most 512 KiB (`attachments.rs`).
public enum ImagePreparer {
    public static let maximumBytes = 512 * 1024
    public static let maximumImages = 5

    public enum Failure: Error, Equatable {
        case unreadable
        case tooLarge
    }

    /// Downscale (longest side ≤ 2000 px) and re-encode as JPEG, lowering
    /// quality and then size until it fits.
    public static func jpeg(from data: Data, maximumBytes: Int = maximumBytes) throws -> Data {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { throw Failure.unreadable }
        var side: CGFloat = 2000
        for _ in 0..<6 {
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: side,
            ]
            guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else {
                throw Failure.unreadable
            }
            let flat = flatten(image) ?? image
            for quality in [0.85, 0.7, 0.5, 0.3] {
                if let encoded = encode(flat, quality: quality), encoded.count <= maximumBytes { return encoded }
            }
            side *= 0.7
        }
        throw Failure.tooLarge
    }

    /// JPEG has no alpha: draw on white so transparent areas stay legible.
    static func flatten(_ image: CGImage) -> CGImage? {
        guard let context = CGContext(
            data: nil, width: image.width, height: image.height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
        ) else { return nil }
        let rect = CGRect(x: 0, y: 0, width: image.width, height: image.height)
        context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        context.fill(rect)
        context.draw(image, in: rect)
        return context.makeImage()
    }

    static func encode(_ image: CGImage, quality: Double) -> Data? {
        let output = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(output, UTType.jpeg.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
        guard CGImageDestinationFinalize(destination) else { return nil }
        return output as Data
    }
}
