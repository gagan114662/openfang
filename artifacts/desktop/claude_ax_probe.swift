import Cocoa
import ApplicationServices

func attr<T>(_ element: AXUIElement, _ name: String) -> T? {
    var value: CFTypeRef?
    let err = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    guard err == .success, let v = value else { return nil }
    return v as? T
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
    let role: String
    let title: String
    let desc: String
    let x: CGFloat
    let y: CGFloat
    let width: CGFloat
    let height: CGFloat
}

func collect(_ element: AXUIElement, depth: Int = 0, maxDepth: Int = 24, into results: inout [Candidate]) {
    let role: String = attr(element, kAXRoleAttribute as String) ?? ""
    let title: String = attr(element, kAXTitleAttribute as String) ?? ""
    let desc: String = attr(element, kAXDescriptionAttribute as String) ?? ""
    let point = axPoint(element, kAXPositionAttribute as String) ?? .zero
    let size = axSize(element, kAXSizeAttribute as String) ?? .zero
    results.append(
        Candidate(
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

let apps = NSRunningApplication.runningApplications(withBundleIdentifier: "com.google.Chrome")
guard let app = apps.first else {
    print("{\"success\":false,\"error\":\"Chrome not running\"}")
    exit(1)
}
let appElem = AXUIElementCreateApplication(app.processIdentifier)
var windowRef: CFTypeRef?
let windowErr = AXUIElementCopyAttributeValue(appElem, kAXFocusedWindowAttribute as CFString, &windowRef)
guard windowErr == .success, let focusedWindow = windowRef else {
    print("{\"success\":false,\"error\":\"No focused Chrome window\",\"ax_error\":\(windowErr.rawValue)}")
    exit(1)
}
let root = focusedWindow as! AXUIElement
let windowPos = axPoint(root, kAXPositionAttribute as String) ?? .zero
let windowSize = axSize(root, kAXSizeAttribute as String) ?? .zero
let midX = windowPos.x + (windowSize.width * 0.60)
var candidates: [Candidate] = []
collect(root, into: &candidates)

let rightPane = candidates.filter { $0.x >= midX && $0.width > 0 }
let textAreas = rightPane.filter { $0.role == "AXTextArea" || $0.role == "AXTextField" }
let buttons = rightPane.filter { $0.role == "AXButton" }
let closeSidePanel = buttons.contains {
    let hay = "\($0.title) \($0.desc)".lowercased()
    return hay.contains("close side panel")
}
let sendButton = buttons.contains {
    let hay = "\($0.title) \($0.desc)".lowercased()
    return hay.contains("send message") || hay == "send"
}
let panelSignals = rightPane.contains {
    let hay = "\($0.title) \($0.desc)".lowercased()
    return hay.contains("act without asking")
        || hay.contains("how can i help you")
        || hay.contains("type / for commands")
        || hay.contains("stop claude")
}
let panelVisible = panelSignals || closeSidePanel || !textAreas.isEmpty
let payload: [String: Any] = [
    "success": true,
    "panel_visible": panelVisible,
    "composer_found": !textAreas.isEmpty,
    "send_button_found": sendButton,
    "close_side_panel_found": closeSidePanel,
    "window_width": windowSize.width,
    "window_height": windowSize.height,
    "candidate_count": candidates.count,
]
let data = try JSONSerialization.data(withJSONObject: payload)
print(String(data: data, encoding: .utf8)!)
