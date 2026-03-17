import Cocoa
import ApplicationServices

func attr<T>(_ element: AXUIElement, _ name: String) -> T? {
    var value: CFTypeRef?
    let err = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    guard err == .success, let v = value else { return nil }
    return v as? T
}

func findFirst(_ element: AXUIElement, where predicate: (AXUIElement) -> Bool, depth: Int = 0, maxDepth: Int = 16) -> AXUIElement? {
    if predicate(element) { return element }
    guard depth < maxDepth else { return nil }
    if let children: [AXUIElement] = attr(element, kAXChildrenAttribute as String) {
        for child in children {
            if let found = findFirst(child, where: predicate, depth: depth + 1, maxDepth: maxDepth) {
                return found
            }
        }
    }
    return nil
}

func axPoint(_ element: AXUIElement, _ name: String) -> CGPoint? {
    var value: CFTypeRef?
    let err = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    guard err == .success, let raw = value, CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    let axValue = raw as! AXValue
    guard AXValueGetType(axValue) == .cgPoint else { return nil }
    var point = CGPoint.zero
    return AXValueGetValue(axValue, .cgPoint, &point) ? point : nil
}

func axSize(_ element: AXUIElement, _ name: String) -> CGSize? {
    var value: CFTypeRef?
    let err = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    guard err == .success, let raw = value, CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    let axValue = raw as! AXValue
    guard AXValueGetType(axValue) == .cgSize else { return nil }
    var size = CGSize.zero
    return AXValueGetValue(axValue, .cgSize, &size) ? size : nil
}

struct Candidate {
    let element: AXUIElement
    let role: String
    let title: String
    let desc: String
    let x: CGFloat
    let y: CGFloat
    let width: CGFloat
    let height: CGFloat
}

func collect(_ element: AXUIElement, depth: Int = 0, maxDepth: Int = 20, into results: inout [Candidate]) {
    let role: String = attr(element, kAXRoleAttribute as String) ?? ""
    let title: String = attr(element, kAXTitleAttribute as String) ?? ""
    let desc: String = attr(element, kAXDescriptionAttribute as String) ?? ""
    let point = axPoint(element, kAXPositionAttribute as String) ?? .zero
    let size = axSize(element, kAXSizeAttribute as String) ?? .zero
    results.append(
        Candidate(
            element: element,
            role: role,
            title: title,
            desc: desc,
            x: point.x,
            y: point.y,
            width: size.width,
            height: size.height
        )
    )
    guard depth < maxDepth else { return }
    if let children: [AXUIElement] = attr(element, kAXChildrenAttribute as String) {
        for child in children {
            collect(child, depth: depth + 1, maxDepth: maxDepth, into: &results)
        }
    }
}

let text = CommandLine.arguments.dropFirst().joined(separator: " ")
let apps = NSRunningApplication.runningApplications(withBundleIdentifier: "com.google.Chrome")
guard let app = apps.first else {
    print("{\"success\":false,\"error\":\"Chrome not running\"}")
    exit(1)
}
let appElem = AXUIElementCreateApplication(app.processIdentifier)
var windowRef: CFTypeRef?
let windowErr = AXUIElementCopyAttributeValue(appElem, kAXFocusedWindowAttribute as CFString, &windowRef)
guard windowErr == .success, let focusedWindow = windowRef else {
    print("{\"success\":false,\"error\":\"No focused Chrome window\"}")
    exit(1)
}
let root = focusedWindow as! AXUIElement
let windowPos = axPoint(root, kAXPositionAttribute as String) ?? .zero
let windowSize = axSize(root, kAXSizeAttribute as String) ?? .zero
let midX = windowPos.x + (windowSize.width * 0.60)

var candidates: [Candidate] = []
collect(root, into: &candidates)

let textAreas = candidates.filter { candidate in
    (candidate.role == "AXTextArea" || candidate.role == "AXTextField") &&
    candidate.x >= midX &&
    candidate.width >= 120
}

guard let textArea = textAreas.sorted(by: {
    if abs($0.y - $1.y) > 2 { return $0.y > $1.y }
    if abs($0.x - $1.x) > 2 { return $0.x > $1.x }
    return $0.width > $1.width
}).first else {
    print("{\"success\":false,\"error\":\"Claude AXTextArea not found\",\"candidate_count\":\(candidates.count),\"window_mid_x\":\(midX)}")
    exit(1)
}

let focusSetErr = AXUIElementSetAttributeValue(textArea.element, kAXFocusedAttribute as CFString, kCFBooleanTrue)
let valueSetErr = AXUIElementSetAttributeValue(textArea.element, kAXValueAttribute as CFString, text as CFTypeRef)

let sendButtons = candidates.filter { candidate in
    guard candidate.role == "AXButton", candidate.x >= midX else { return false }
    let haystack = "\(candidate.title) \(candidate.desc)".lowercased()
    return haystack.contains("send message") || haystack == "send"
}

guard let sendButton = sendButtons.sorted(by: {
    if abs($0.y - $1.y) > 2 { return $0.y > $1.y }
    if abs($0.x - $1.x) > 2 { return $0.x > $1.x }
    return $0.width > $1.width
}).first else {
    print("{\"success\":false,\"error\":\"Claude send button not found\",\"focus_err\":\(focusSetErr.rawValue),\"value_err\":\(valueSetErr.rawValue),\"candidate_count\":\(candidates.count)}")
    exit(1)
}

let pressErr = AXUIElementPerformAction(sendButton.element, kAXPressAction as CFString)
print("{\"success\":\(focusSetErr == .success && valueSetErr == .success && pressErr == .success),\"focus_err\":\(focusSetErr.rawValue),\"value_err\":\(valueSetErr.rawValue),\"press_err\":\(pressErr.rawValue),\"text_area_x\":\(textArea.x),\"text_area_y\":\(textArea.y),\"send_x\":\(sendButton.x),\"send_y\":\(sendButton.y)}")
