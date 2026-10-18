#!/usr/bin/env swift
// macOS Vision Framework OCR — zero-dependency, built into macOS 10.15+
// Usage: swift vision_ocr.swift <image_path>
// Output: JSON array of {text, confidence, x, y, w, h}

import Foundation
import Vision
import AppKit

guard CommandLine.arguments.count >= 2 else {
    fputs("Usage: vision_ocr <image_path>\n", stderr)
    exit(1)
}

let imagePath = CommandLine.arguments[1]
guard let image = NSImage(contentsOfFile: imagePath),
      let tiffData = image.tiffRepresentation,
      let bitmap = NSBitmapImageRep(data: tiffData),
      let cgImage = bitmap.cgImage else {
    fputs("Failed to load image: \(imagePath)\n", stderr)
    // Output empty array so caller doesn't crash
    print("[]")
    exit(0)
}

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.recognitionLanguages = ["en", "ja", "zh-Hans", "zh-Hant", "ko"]
request.usesLanguageCorrection = true

let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])

do {
    try handler.perform([request])
} catch {
    fputs("Vision OCR error: \(error)\n", stderr)
    print("[]")
    exit(0)
}

var results: [[String: Any]] = []

if let observations = request.results {
    let imageWidth = CGFloat(cgImage.width)
    let imageHeight = CGFloat(cgImage.height)

    for observation in observations {
        guard let topCandidate = observation.topCandidates(1).first else { continue }

        let bbox = observation.boundingBox
        let x = Int(bbox.origin.x * imageWidth)
        let y = Int((1.0 - bbox.origin.y - bbox.height) * imageHeight)
        let w = Int(bbox.width * imageWidth)
        let h = Int(bbox.height * imageHeight)

        results.append([
            "text": topCandidate.string,
            "confidence": round(topCandidate.confidence * 1000) / 1000,
            "x": x, "y": y, "w": w, "h": h,
        ])
    }
}

// Sort top-to-bottom, then left-to-right (manga reading order)
results.sort { a, b in
    let ay = a["y"] as! Int
    let by = b["y"] as! Int
    if abs(ay - by) > 30 { return ay < by }
    let ax = a["x"] as! Int
    let bx = b["x"] as! Int
    return ax < bx
}

if let jsonData = try? JSONSerialization.data(withJSONObject: results, options: []),
   let jsonString = String(data: jsonData, encoding: .utf8) {
    print(jsonString)
} else {
    print("[]")
}
