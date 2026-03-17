import Foundation
import Vision
import AppKit

let path = CommandLine.arguments[1]
let url = URL(fileURLWithPath: path)
let image = NSImage(contentsOf: url)
var rect = NSRect.zero
guard let cgImage = image?.cgImage(forProposedRect: &rect, context: nil, hints: nil) else {
    exit(1)
}
let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = false
let handler = VNImageRequestHandler(cgImage: cgImage)
try handler.perform([request])
let observations = request.results ?? []
let width = CGFloat(cgImage.width)
let height = CGFloat(cgImage.height)
for observation in observations.prefix(80) {
    if let top = observation.topCandidates(1).first {
        let box = observation.boundingBox
        let obj: [String: Any] = [
            "text": top.string,
            "x": box.origin.x * width,
            "y": (1 - box.origin.y - box.size.height) * height,
            "w": box.size.width * width,
            "h": box.size.height * height
        ]
        let data = try JSONSerialization.data(withJSONObject: obj)
        print(String(data: data, encoding: .utf8)!)
    }
}
