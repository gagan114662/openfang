#!/usr/bin/env python3
"""Small CLI wrapper around the OpenFang macOS desktop bridge.

Examples:
  python3 scripts/desktop_control.py launch Calculator
  python3 scripts/desktop_control.py type "123+456"
  python3 scripts/desktop_control.py calc-input "123+456"
  python3 scripts/desktop_control.py open-url "https://foolish.sentry.io"
  python3 scripts/desktop_control.py active-window
  python3 scripts/desktop_control.py screenshot
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from base64 import b64decode
from pathlib import Path

from Quartz import CGWindowListCopyWindowInfo, kCGNullWindowID, kCGWindowListOptionOnScreenOnly


ROOT = Path(__file__).resolve().parents[1]
BRIDGE = ROOT / "crates" / "openfang-runtime" / "src" / "desktop_bridge.py"
BROWSER_BRIDGE = ROOT / "crates" / "openfang-runtime" / "src" / "browser_bridge.py"
CHROME_EXECUTABLE = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
CHROME_PROFILE = "/Users/gaganarora/Library/Application Support/Google/Chrome/Default"
SENTRY_URL = "https://foolish.sentry.io/issues/?project=-1&statsPeriod=24h"
CLAUDE_EXTENSION_ID = "fcoeoabgfenejglbffodgkkbkcdhcgfn"
CLAUDE_EXTENSION_OPTIONS_URL = f"chrome-extension://{CLAUDE_EXTENSION_ID}/options.html"
CLAUDE_EXTENSION_SIDEPANEL_URL = f"chrome-extension://{CLAUDE_EXTENSION_ID}/sidepanel.html"
CLAUDE_LIVE_URL = "https://claude.ai/new"
FFMPEG_BIN = "/opt/homebrew/bin/ffmpeg"
SCREEN_CAPTURE_DEVICE = "1:none"
OCR_TOOL_DIR = ROOT / "artifacts" / "desktop"
OCR_TOOL_SOURCE = OCR_TOOL_DIR / "ocr_image_lines.swift"
OCR_TOOL_BINARY = OCR_TOOL_DIR / "ocr_image_lines"
CLAUDE_AX_TOOL_SOURCE = OCR_TOOL_DIR / "claude_ax_prompt.swift"
CLAUDE_AX_TOOL_BINARY = OCR_TOOL_DIR / "claude_ax_prompt"
CLAUDE_PANEL_PROBE_SOURCE = OCR_TOOL_DIR / "claude_ax_probe.swift"
CLAUDE_PANEL_PROBE_BINARY = OCR_TOOL_DIR / "claude_ax_probe"

CALC_BUTTONS = {
    "AC": (31.5, 123.0),
    "+/-": (79.0, 123.0),
    "%": (127.5, 123.0),
    "/": (177.5, 123.0),
    "7": (31.5, 173.0),
    "8": (79.0, 173.0),
    "9": (127.5, 173.0),
    "*": (177.5, 173.0),
    "x": (177.5, 173.0),
    "4": (31.5, 223.0),
    "5": (79.0, 223.0),
    "6": (127.5, 223.0),
    "-": (177.5, 223.0),
    "1": (31.5, 273.0),
    "2": (79.0, 273.0),
    "3": (127.5, 273.0),
    "+": (177.5, 273.0),
    "0": (79.0, 323.0),
    ".": (127.5, 323.0),
    "=": (177.5, 323.0),
}


def find_calculator_window_bounds() -> tuple[float, float, float, float]:
    windows = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID)
    for window in windows:
        if window.get("kCGWindowOwnerName") == "Calculator":
            bounds = window.get("kCGWindowBounds", {})
            return (
                float(bounds.get("X", 0.0)),
                float(bounds.get("Y", 0.0)),
                float(bounds.get("Width", 0.0)),
                float(bounds.get("Height", 0.0)),
            )
    raise SystemExit("Calculator window not found")


def find_window_bounds(owner_name: str, title_hint: str | None = None) -> tuple[float, float, float, float]:
    windows = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID)
    title_hint = (title_hint or "").strip().lower()
    fallback = None
    for window in windows:
        if window.get("kCGWindowOwnerName") != owner_name:
            continue
        bounds = window.get("kCGWindowBounds", {})
        rect = (
            float(bounds.get("X", 0.0)),
            float(bounds.get("Y", 0.0)),
            float(bounds.get("Width", 0.0)),
            float(bounds.get("Height", 0.0)),
        )
        if fallback is None:
            fallback = rect
        title = str(window.get("kCGWindowName") or "").lower()
        if title_hint and title_hint in title:
            return rect
    if fallback is not None:
        return fallback
    raise SystemExit(f"{owner_name} window not found")


def run_bridge(actions: list[dict]) -> list[dict]:
    payload = "\n".join(json.dumps(a) for a in actions + [{"action": "Close"}]) + "\n"
    proc = subprocess.run(
        [sys.executable, str(BRIDGE)],
        input=payload,
        text=True,
        capture_output=True,
        check=False,
    )
    if proc.returncode != 0:
        raise SystemExit(proc.stderr.strip() or f"desktop bridge exited with {proc.returncode}")

    lines = [line for line in proc.stdout.splitlines() if line.strip()]
    if not lines:
        raise SystemExit("desktop bridge returned no output")

    responses = [json.loads(line) for line in lines]
    if not responses[0].get("success"):
        raise SystemExit(responses[0].get("error") or "desktop bridge failed to start")
    action_responses = responses[1:]
    if action_responses and action_responses[-1].get("data", {}).get("status") == "closed":
        action_responses = action_responses[:-1]
    return action_responses


def run_browser_probe(url: str) -> dict:
    cmd = [
        sys.executable,
        str(BROWSER_BRIDGE),
        "--headless",
        "--user-data-dir",
        CHROME_PROFILE,
        "--browser-executable",
        CHROME_EXECUTABLE,
        "--timeout",
        "15",
    ]
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        ready = proc.stdout.readline().strip()
        if not ready:
            raise RuntimeError("browser bridge returned no ready signal")
        ready_obj = json.loads(ready)
        if not ready_obj.get("success"):
            raise RuntimeError(ready_obj.get("error") or "browser bridge failed to start")
        proc.stdin.write(json.dumps({"action": "Navigate", "url": url}) + "\n")
        proc.stdin.flush()
        response_line = proc.stdout.readline().strip()
        if not response_line:
            raise RuntimeError("browser bridge returned no navigate response")
        response = json.loads(response_line)
        if not response.get("success"):
            raise RuntimeError(response.get("error") or "browser probe navigate failed")
        data = response.get("data") or {}
        content = (data.get("content") or "").strip()
        return {
            "success": True,
            "title": data.get("title"),
            "url": data.get("url"),
            "content_excerpt": content[:3000],
        }
    except Exception as exc:
        return {"success": False, "error": str(exc)}
    finally:
        try:
            if proc.stdin and not proc.stdin.closed:
                proc.stdin.write(json.dumps({"action": "Close"}) + "\n")
                proc.stdin.flush()
                proc.stdin.close()
        except Exception:
            pass
        try:
            if proc.stdout and not proc.stdout.closed:
                proc.stdout.close()
        except Exception:
            pass
        try:
            if proc.stderr and not proc.stderr.closed:
                proc.stderr.close()
        except Exception:
            pass
        try:
            if proc.poll() is None:
                proc.kill()
        except Exception:
            pass


def chrome_tab_value(expression: str) -> str | None:
    try:
        result = subprocess.run(
            ["osascript", "-e", expression],
            capture_output=True,
            text=True,
            check=False,
            timeout=2.0,
        )
    except subprocess.TimeoutExpired:
        return None
    if result.returncode != 0:
        return None
    value = result.stdout.strip()
    return value or None


def url_matches_target(actual: str | None, target: str) -> bool:
    if not actual:
        return False
    normalized_actual = actual.rstrip("/")
    normalized_target = target.rstrip("/")
    return (
        normalized_actual == normalized_target
        or normalized_actual.startswith(normalized_target + "#")
    )


def chrome_set_front_tab_url(url: str) -> bool:
    script = f'''
tell application "Google Chrome"
  activate
  if (count of windows) = 0 then make new window
  set URL of active tab of front window to "{url}"
end tell
'''
    try:
        result = subprocess.run(
            ["osascript", "-e", script],
            capture_output=True,
            text=True,
            check=False,
            timeout=3.0,
        )
        if result.returncode == 0:
            ensure_frontmost_app("Google Chrome", attempts=6, delay=0.3)
            current_url = chrome_tab_value(
                'tell application "Google Chrome" to get URL of active tab of front window'
            )
            if url_matches_target(current_url, url):
                return True
    except subprocess.TimeoutExpired:
        pass

    subprocess.run(
        ["open", "-a", "Google Chrome", url],
        capture_output=True,
        text=True,
        check=False,
    )
    time.sleep(1.0)
    if not ensure_frontmost_app("Google Chrome", attempts=8, delay=0.35):
        return False
    try:
        run_bridge(
            [
                {"action": "KeyPress", "key": "l", "modifiers": ["command"]},
                {"action": "Type", "text": url},
                {"action": "KeyPress", "key": "return", "modifiers": []},
            ]
        )
        time.sleep(1.5)
    except Exception:
        pass
    current_url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    ) or ""
    if url_matches_target(current_url, url):
        return True

    fallback_script = f'''
tell application "Google Chrome"
  activate
  if (count of windows) = 0 then make new window
  tell front window
    set newTab to make new tab with properties {{URL:"{url}"}}
    set active tab index to (count of tabs)
  end tell
end tell
'''
    subprocess.run(
        ["osascript", "-e", fallback_script],
        capture_output=True,
        text=True,
        check=False,
    )
    time.sleep(1.2)
    current_url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    ) or ""
    return url_matches_target(current_url, url)


def active_window_data() -> dict:
    responses = run_bridge([{"action": "GetActiveWindow"}])
    return responses[0].get("data") or {}


def ensure_frontmost_app(app_name: str, attempts: int = 4, delay: float = 0.25) -> bool:
    for _ in range(max(1, attempts)):
        activate_app(app_name)
        time.sleep(delay)
        active = active_window_data()
        if active.get("app_name") == app_name:
            return True
    return False


def open_permission_settings(permission_kind: str) -> None:
    pane_map = {
        "accessibility": "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        "screen_capture": "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    }
    url = pane_map.get(permission_kind)
    if not url:
        return
    subprocess.run(["open", url], capture_output=True, text=True, check=False)


def accessibility_trusted() -> tuple[bool, str | None]:
    # Prefer the macOS ApplicationServices API when available.
    try:
        from ApplicationServices import AXIsProcessTrusted

        trusted = bool(AXIsProcessTrusted())
        return trusted, None if trusted else "Accessibility permission is not granted."
    except Exception as exc:
        # Some Python runtimes in this repo do not ship ApplicationServices,
        # while the desktop bridge can still drive AX through PyObjC.
        # Treat this as an unknown state and let active-window/screenshot gates
        # determine preflight readiness.
        return True, f"Accessibility trust check unavailable in this runtime: {exc}"


def desktop_preflight() -> dict:
    blockers: list[dict] = []
    active = {}
    screen = {"width": 0, "height": 0}

    try:
        active = active_window_data()
    except Exception as exc:
        blockers.append(
            {
                "kind": "active_window",
                "reason": f"Failed reading active window: {exc}",
                "settings": None,
            }
        )

    app_name = str(active.get("app_name") or "").strip()
    if not app_name:
        blockers.append(
            {
                "kind": "active_window",
                "reason": "No frontmost active window is available.",
                "settings": None,
            }
        )

    try:
        screen = screen_size_data()
    except Exception as exc:
        blockers.append(
            {
                "kind": "screen_size",
                "reason": f"Failed reading screen size: {exc}",
                "settings": None,
            }
        )

    width = int(screen.get("width") or 0)
    height = int(screen.get("height") or 0)
    if width <= 0 or height <= 0:
        blockers.append(
            {
                "kind": "screen_size",
                "reason": f"Invalid screen size ({width}x{height}).",
                "settings": None,
            }
        )

    ax_ok, ax_reason = accessibility_trusted()
    if not ax_ok:
        blockers.append(
            {
                "kind": "accessibility",
                "reason": ax_reason or "Accessibility permission is not granted.",
                "settings": "accessibility",
            }
        )

    screenshot_probe = {"success": False, "error": "unknown"}
    try:
        screenshot_probe = run_bridge([{"action": "Screenshot"}])[0]
    except Exception as exc:
        screenshot_probe = {"success": False, "error": str(exc)}
    if not screenshot_probe.get("success"):
        blockers.append(
            {
                "kind": "screen_capture",
                "reason": screenshot_probe.get("error")
                or "Screen Recording permission is not granted.",
                "settings": "screen_capture",
            }
        )

    for blocker in blockers:
        setting_kind = blocker.get("settings")
        if isinstance(setting_kind, str):
            open_permission_settings(setting_kind)

    return {
        "ok": not blockers,
        "failure_phase": "preflight" if blockers else None,
        "failure_reason": blockers[0]["reason"] if blockers else None,
        "active_window": active,
        "screen": {"width": width, "height": height},
        "screenshot_probe": screenshot_probe,
        "blockers": blockers,
    }


def screenshot_data() -> dict:
    responses = run_bridge([{"action": "Screenshot"}])
    final = responses[0]
    data = final.get("data") or {}
    artifact_dir = ROOT / "artifacts" / "desktop"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    filename = artifact_dir / f"screenshot-{int(time.time() * 1000)}.png"
    image_b64 = data.get("image_base64")
    if image_b64:
        filename.write_bytes(b64decode(image_b64))
    return {
        "success": final.get("success"),
        "width": data.get("width"),
        "height": data.get("height"),
        "path": str(filename),
    }


def screen_size_data() -> dict:
    responses = run_bridge([{"action": "ScreenSize"}])
    final = responses[0]
    return final.get("data") or {}


def screen_size_data() -> dict:
    responses = run_bridge([{"action": "GetScreenSize"}])
    final = responses[0]
    data = final.get("data") or {}
    return {
        "success": final.get("success"),
        "width": data.get("width"),
        "height": data.get("height"),
    }


def screen_record_data(seconds: float) -> dict:
    artifact_dir = ROOT / "artifacts" / "desktop"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    filename = artifact_dir / f"screen-record-{int(time.time() * 1000)}.mp4"
    duration = max(1.0, min(seconds, 30.0))
    if not os.path.exists(FFMPEG_BIN):
        return {"success": False, "error": f"ffmpeg not found at {FFMPEG_BIN}"}
    cmd = [
        FFMPEG_BIN,
        "-y",
        "-f",
        "avfoundation",
        "-capture_cursor",
        "1",
        "-framerate",
        "15",
        "-i",
        SCREEN_CAPTURE_DEVICE,
        "-t",
        f"{duration}",
        "-pix_fmt",
        "yuv420p",
        str(filename),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, check=False)
    success = result.returncode == 0 and filename.exists()
    if not success:
        stderr = (result.stderr or "").strip()
        return {
            "success": False,
            "error": stderr or f"ffmpeg exited with status {result.returncode}",
        }
    return {
        "success": True,
        "path": str(filename),
        "filename": filename.name,
        "duration_secs": duration,
        "size_bytes": filename.stat().st_size,
    }


def ocr_image_lines(path: str) -> list[dict]:
    swift_source = """\
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
"""
    OCR_TOOL_DIR.mkdir(parents=True, exist_ok=True)
    current_source = OCR_TOOL_SOURCE.read_text() if OCR_TOOL_SOURCE.exists() else ""
    if current_source != swift_source:
        OCR_TOOL_SOURCE.write_text(swift_source)
    if (not OCR_TOOL_BINARY.exists()) or OCR_TOOL_BINARY.stat().st_mtime < OCR_TOOL_SOURCE.stat().st_mtime:
        compile_result = subprocess.run(
            ["swiftc", "-O", str(OCR_TOOL_SOURCE), "-o", str(OCR_TOOL_BINARY)],
            capture_output=True,
            text=True,
            check=False,
        )
        if compile_result.returncode != 0:
            return []
    try:
        result = subprocess.run(
            [str(OCR_TOOL_BINARY), path],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            return []
        lines: list[dict] = []
        for raw in result.stdout.splitlines():
            raw = raw.strip()
            if not raw:
                continue
            try:
                parsed = json.loads(raw)
            except json.JSONDecodeError:
                continue
            lines.append(parsed)
        return lines
    finally:
        pass


def ocr_image(path: str) -> str:
    return "\n".join(
        str(line.get("text", "")).strip() for line in ocr_image_lines(path) if str(line.get("text", "")).strip()
    )


def ensure_claude_ax_tool() -> bool:
    OCR_TOOL_DIR.mkdir(parents=True, exist_ok=True)
    swift_source = r'''import Cocoa
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
'''
    current_source = CLAUDE_AX_TOOL_SOURCE.read_text() if CLAUDE_AX_TOOL_SOURCE.exists() else ""
    if current_source != swift_source:
        CLAUDE_AX_TOOL_SOURCE.write_text(swift_source)
    if (not CLAUDE_AX_TOOL_BINARY.exists()) or CLAUDE_AX_TOOL_BINARY.stat().st_mtime < CLAUDE_AX_TOOL_SOURCE.stat().st_mtime:
        compile_result = subprocess.run(
            ["swiftc", "-O", str(CLAUDE_AX_TOOL_SOURCE), "-o", str(CLAUDE_AX_TOOL_BINARY)],
            capture_output=True,
            text=True,
            check=False,
        )
        if compile_result.returncode != 0:
            return False
    return True


def claude_ax_prompt(text: str) -> dict:
    if not ensure_claude_ax_tool():
        return {"success": False, "error": "Failed compiling Claude AX helper"}
    result = subprocess.run(
        [str(CLAUDE_AX_TOOL_BINARY), text],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0 and not result.stdout.strip():
        return {"success": False, "error": result.stderr.strip() or "Claude AX helper failed"}
    try:
        return json.loads(result.stdout.strip())
    except json.JSONDecodeError:
        return {"success": False, "error": result.stdout.strip() or result.stderr.strip() or "Claude AX helper returned invalid JSON"}


def ensure_claude_panel_probe_tool() -> bool:
    OCR_TOOL_DIR.mkdir(parents=True, exist_ok=True)
    swift_source = r'''import Cocoa
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
'''
    current_source = (
        CLAUDE_PANEL_PROBE_SOURCE.read_text()
        if CLAUDE_PANEL_PROBE_SOURCE.exists()
        else ""
    )
    if current_source != swift_source:
        CLAUDE_PANEL_PROBE_SOURCE.write_text(swift_source)
    if (not CLAUDE_PANEL_PROBE_BINARY.exists()) or (
        CLAUDE_PANEL_PROBE_BINARY.stat().st_mtime < CLAUDE_PANEL_PROBE_SOURCE.stat().st_mtime
    ):
        compile_result = subprocess.run(
            ["swiftc", "-O", str(CLAUDE_PANEL_PROBE_SOURCE), "-o", str(CLAUDE_PANEL_PROBE_BINARY)],
            capture_output=True,
            text=True,
            check=False,
        )
        if compile_result.returncode != 0:
            return False
    return True


def claude_ax_probe() -> dict:
    if not ensure_claude_panel_probe_tool():
        return {"success": False, "error": "Failed compiling Claude AX probe helper"}
    result = subprocess.run(
        [str(CLAUDE_PANEL_PROBE_BINARY)],
        capture_output=True,
        text=True,
        check=False,
    )
    stdout = (result.stdout or "").strip()
    if result.returncode != 0 and not stdout:
        return {"success": False, "error": result.stderr.strip() or "Claude AX probe failed"}
    try:
        return json.loads(stdout)
    except json.JSONDecodeError:
        return {
            "success": False,
            "error": stdout or result.stderr.strip() or "Claude AX probe returned invalid JSON",
        }


def image_size(path: str) -> tuple[float, float] | None:
    try:
        result = subprocess.run(
            [
                "python3",
                "-c",
                (
                    "from PIL import Image; "
                    "import sys; "
                    "img = Image.open(sys.argv[1]); "
                    "print(f'{img.size[0]} {img.size[1]}')"
                ),
                path,
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            return None
        width_str, height_str = result.stdout.strip().split()
        return (float(width_str), float(height_str))
    except Exception:
        return None


def crop_image(path: str, left: int, top: int, right: int, bottom: int, stem: str) -> str | None:
    try:
        from PIL import Image
    except Exception:
        return None
    try:
        image = Image.open(path)
        cropped = image.crop((left, top, right, bottom))
        dest = OCR_TOOL_DIR / f"{stem}-{int(time.time() * 1000)}.png"
        cropped.save(dest)
        return str(dest)
    except Exception:
        return None


def claude_panel_ocr_text(screenshot: dict) -> str:
    screenshot_path = str(screenshot.get("path") or "")
    width = int(float(screenshot.get("width") or 0.0))
    height = int(float(screenshot.get("height") or 0.0))
    if not screenshot_path or width <= 0 or height <= 0:
        return ""
    crop_path = crop_image(
        screenshot_path,
        max(0, int(width * 0.70)),
        0,
        width,
        height,
        "claude-panel",
    )
    if not crop_path:
        return ""
    return ocr_image(crop_path)


def significant_panel_lines(text: str, prompt_text: str = "") -> list[str]:
    prompt_norm = " ".join(prompt_text.lower().split())
    lines: list[str] = []
    for raw_line in text.splitlines():
        line = " ".join(raw_line.split()).strip()
        lower = line.lower()
        if len(line) < 6:
            continue
        if prompt_norm and lower in prompt_norm:
            continue
        if prompt_norm and lower in prompt_norm[: max(32, min(len(prompt_norm), 96))]:
            continue
        if any(
            fragment in lower
            for fragment in (
                "high risk:",
                "see safe use tips",
                "act without asking",
                "claude is ai and can make mistakes",
                "reply to claude",
                "wallet empty",
                "add credits",
                "sonnet 4.6",
                "claude",
                "ask gemini",
                "work",
                "all bookmarks",
            )
        ):
            continue
        lines.append(line)
    return lines


def extract_new_panel_response(before_text: str, after_text: str, prompt_text: str) -> str | None:
    before_lines = significant_panel_lines(before_text, prompt_text)
    after_lines = significant_panel_lines(after_text, prompt_text)
    before_set = set(before_lines)
    new_lines = [line for line in after_lines if line not in before_set]
    if not new_lines:
        return None
    return "\n".join(new_lines[:8]).strip() or None


def normalize_image_point(
    point: tuple[float, float],
    screenshot_path: str,
    screen_width: float | None = None,
    screen_height: float | None = None,
) -> tuple[float, float]:
    logical_width = float(screen_width or 0.0)
    logical_height = float(screen_height or 0.0)
    image_dims = image_size(screenshot_path)
    if (
        image_dims is None
        or logical_width <= 0.0
        or logical_height <= 0.0
    ):
        return point
    image_width, image_height = image_dims
    if image_width <= 0.0 or image_height <= 0.0:
        return point
    scale_x = image_width / logical_width
    scale_y = image_height / logical_height
    if scale_x <= 0.0 or scale_y <= 0.0:
        return point
    return (point[0] / scale_x, point[1] / scale_y)


def infer_page_hint(url: str | None, title: str | None, ocr_text: str) -> str:
    haystack = " ".join(filter(None, [url or "", title or "", ocr_text])).lower()
    if (
        "how can i help you today" in haystack
        and ("act without asking" in haystack or "type / for commands" in haystack)
        and "sonnet 4.6" in haystack
    ):
        return "claude_extension_panel"
    if "claude.ai/chat/" in haystack:
        return "claude_live"
    if "claude.ai/" in haystack and (
        "how can i help you today" in haystack
        or "claude for chrome" in haystack
        or "evening," in haystack
        or "reply..." in haystack
        or "thinking about" in haystack
        or "want to be notified when claude responds" in haystack
        or "relevant chats" in haystack
    ):
        return "claude_live"
    if "chrome-extension://" in haystack and "claude" in haystack:
        return "claude_extension"
    if "auth/login" in haystack or "login with google" in haystack or "sign in" in haystack:
        return "login_required"
    if "no issues match your search" in haystack:
        return "empty_issues_page"
    if "sentry" in haystack and "issue" in haystack:
        return "issues_page"
    if "sentry" in haystack:
        return "sentry_page"
    return "unknown"


def extract_issue_titles(ocr_text: str) -> list[str]:
    if "no issues match your search" in ocr_text.lower():
        return []
    ignore_fragments = {
        "telegram",
        "openclaw demo",
        "update telegram",
        "botfather",
        "new thread",
        "automations",
        "skills",
        "file",
        "edit",
        "view",
        "window",
        "help",
        "chats",
        "search",
        "codex",
        "sat mar",
        "ask gemini",
        "all bookmarks",
        "find my device",
        "save as",
        "last seen",
        "tab search is now pinned",
        "you can unpin it",
    }
    issue_fragments = (
        "warning",
        "error",
        "failed",
        "timed out",
        "timeout",
        "response failed",
        "validation",
        "api error",
        "codex cli",
        "claude cli",
        "subprocess",
        "config",
    )
    titles: list[str] = []
    for raw_line in ocr_text.splitlines():
        line = " ".join(raw_line.split()).strip()
        lower = line.lower()
        if len(line) < 8:
            continue
        if any(fragment in lower for fragment in ignore_fragments):
            continue
        if lower.startswith("http"):
            continue
        if not any(fragment in lower for fragment in issue_fragments):
            continue
        if line.startswith("•"):
            line = line.lstrip("•").strip()
            lower = line.lower()
        if "no error message" in lower:
            continue
        if lower.startswith("i ") and len(line) < 32:
            continue
        if len(line) > 160:
            line = f"{line[:157]}..."
        if line in titles:
            continue
        titles.append(line)
        if len(titles) >= 5:
            break
    return titles


def browser_state_payload(include_probe: bool = False, probe_url: str | None = None) -> dict:
    active = active_window_data()
    app_name = active.get("app_name")
    app_is_chrome = app_name == "Google Chrome"
    title = (
        chrome_tab_value('tell application "Google Chrome" to get title of active tab of front window')
        if app_is_chrome
        else None
    )
    url = (
        chrome_tab_value('tell application "Google Chrome" to get URL of active tab of front window')
        if app_is_chrome
        else None
    )
    screenshot = screenshot_data()
    ocr_text = ""
    if app_is_chrome and screenshot.get("success") and screenshot.get("path"):
        ocr_text = ocr_image(screenshot["path"])
    page_hint = infer_page_hint(url, title, ocr_text)
    issue_titles = extract_issue_titles(ocr_text)
    probe = None
    if include_probe and probe_url:
        probe = run_browser_probe(probe_url)
        if probe.get("success"):
            probe_hint = infer_page_hint(probe.get("url"), probe.get("title"), probe.get("content_excerpt", ""))
            if page_hint == "unknown":
                page_hint = probe_hint
    return {
        "success": bool(screenshot.get("success")),
        "data": {
            "app_name": app_name,
            "window_title": title or active.get("window_title"),
            "url": url,
            "page_hint": page_hint,
            "screenshot_path": screenshot.get("path"),
            "width": screenshot.get("width"),
            "height": screenshot.get("height"),
            "page_text_excerpt": ocr_text[:3000],
            "recent_issue_titles": issue_titles,
            "login_required": page_hint == "login_required",
            "probe": probe,
        },
    }


def quit_app(app_name: str) -> None:
    subprocess.run(
        ["osascript", "-e", f'tell application "{app_name}" to quit'],
        capture_output=True,
        check=False,
        text=True,
    )


def activate_app(app_name: str) -> None:
    subprocess.run(
        ["osascript", "-e", f'tell application "{app_name}" to activate'],
        capture_output=True,
        check=False,
        text=True,
    )


def chrome_open_url_live(url: str) -> tuple[bool, dict]:
    subprocess.run(
        ["open", "-a", "Google Chrome"],
        capture_output=True,
        check=False,
        text=True,
    )
    active = {}
    for _ in range(8):
        activate_app("Google Chrome")
        time.sleep(0.4)
        responses = run_bridge([{"action": "GetActiveWindow"}])
        active = responses[-1].get("data") or {}
        if active.get("app_name") == "Google Chrome":
            break
    if active.get("app_name") != "Google Chrome":
        return False, active

    responses = run_bridge(
        [
            {"action": "KeyPress", "key": "l", "modifiers": ["command"]},
            {"action": "Type", "text": url},
            {"action": "KeyPress", "key": "return", "modifiers": []},
        ]
    )
    ok = all(resp.get("success") for resp in responses)
    time.sleep(1.2)
    current_url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    )
    if ok and current_url and (url == current_url or current_url.startswith(url)):
        active = dict(active)
        active["app_name"] = "Google Chrome"
        return True, active

    subprocess.run(
        ["open", "-a", "Google Chrome", url],
        capture_output=True,
        check=False,
        text=True,
    )
    time.sleep(1.2)
    current_url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    )
    ok = bool(current_url and (url == current_url or current_url.startswith(url)))
    if ok:
        active = dict(active)
        active["app_name"] = "Google Chrome"
    return ok, active


def claude_browser_snapshot(active: dict | None = None) -> tuple[dict, str, str]:
    activate_app("Google Chrome")
    time.sleep(0.35)
    screenshot = screenshot_data()
    ocr_text = ""
    page_hint = "unknown"
    if screenshot.get("success") and screenshot.get("path"):
        ocr_text = ocr_image(str(screenshot["path"]))
        page_hint = infer_page_hint(None, active.get("window_title") if active else None, ocr_text)
    return screenshot, ocr_text, page_hint


def chrome_open_claude_extension_panel() -> tuple[bool, dict]:
    subprocess.run(
        [
            "osascript",
            "-e",
            'tell application "Google Chrome" to activate',
            "-e",
            f'tell application "Google Chrome" to tell front window to make new tab with properties {{URL:"{CLAUDE_EXTENSION_SIDEPANEL_URL}"}}',
        ],
        capture_output=True,
        check=False,
        text=True,
    )
    time.sleep(0.8)
    refreshed = run_bridge([{"action": "GetActiveWindow"}])
    active = refreshed[-1].get("data") or {}
    ok = active.get("app_name") == "Google Chrome"
    activate_app("Google Chrome")
    time.sleep(0.35)
    if not ok:
        screenshot, ocr_text, page_hint = claude_browser_snapshot(active)
        current_url = chrome_tab_value(
            'tell application "Google Chrome" to get URL of active tab of front window'
        )
        if page_hint in {"claude_extension_panel", "claude_extension"} or current_url == CLAUDE_EXTENSION_SIDEPANEL_URL:
            active = dict(active)
            active["app_name"] = "Google Chrome"
            active["page_hint"] = page_hint or "claude_extension"
            active["screenshot_path"] = screenshot.get("path")
            active["page_text_excerpt"] = ocr_text[:3000]
            return True, active
        return False, active

    screenshot, ocr_text, page_hint = claude_browser_snapshot(active)
    current_url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    )
    if page_hint in {"claude_extension_panel", "claude_extension"} or current_url == CLAUDE_EXTENSION_SIDEPANEL_URL:
        active = dict(active)
        active["app_name"] = "Google Chrome"
        active["page_hint"] = page_hint or "claude_extension"
        active["screenshot_path"] = screenshot.get("path")
        active["page_text_excerpt"] = ocr_text[:3000]
        return True, active
    return False, active


def chrome_open_claude_sidepanel_on_current_tab() -> tuple[bool, dict]:
    activate_app("Google Chrome")
    time.sleep(0.35)
    before_url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    ) or ""
    attempts = 3
    last_active: dict = {}
    for attempt in range(1, attempts + 1):
        subprocess.run(
            [
                "osascript",
                "-e",
                'tell application "System Events" to keystroke "e" using command down',
            ],
            capture_output=True,
            check=False,
            text=True,
        )
        time.sleep(1.0 + (attempt * 0.25))
        refreshed = run_bridge([{"action": "GetActiveWindow"}])
        active = refreshed[-1].get("data") or {}
        screenshot, ocr_text, page_hint = claude_browser_snapshot(active)
        probe = claude_ax_probe()
        after_url = chrome_tab_value(
            'tell application "Google Chrome" to get URL of active tab of front window'
        ) or ""
        detached = after_url.startswith(f"chrome-extension://{CLAUDE_EXTENSION_ID}/")
        side_visible = bool(probe.get("panel_visible")) or sidepanel_hint_visible(page_hint, ocr_text)
        same_tab = not before_url or after_url == before_url or url_matches_target(after_url, SENTRY_URL)
        last_active = dict(active)
        last_active["current_url"] = after_url
        last_active["before_url"] = before_url
        last_active["attempt_count"] = attempt
        last_active["ax_probe"] = probe
        last_active["claude_attached"] = bool(side_visible and same_tab and not detached)
        last_active["screenshot_path"] = screenshot.get("path")
        last_active["page_text_excerpt"] = ocr_text[:3000]
        if detached:
            last_active["failure_phase"] = "attach_sidepanel"
            last_active["failure_reason"] = "Claude side panel detached into its own tab."
            last_active["error"] = last_active["failure_reason"]
            return False, last_active
        if side_visible and same_tab:
            last_active["page_hint"] = "claude_extension_panel"
            return True, last_active
    if last_active:
        last_active["failure_phase"] = "attach_sidepanel"
        last_active["failure_reason"] = "Claude side panel did not attach to the active Sentry tab."
        last_active["error"] = last_active["failure_reason"]
    return False, last_active


def chrome_open_claude_live() -> tuple[bool, dict]:
    subprocess.run(
        ["open", "-a", "Google Chrome", CLAUDE_LIVE_URL],
        capture_output=True,
        check=False,
        text=True,
    )
    active = {}
    for _ in range(10):
        activate_app("Google Chrome")
        time.sleep(0.5)
        refreshed = run_bridge([{"action": "GetActiveWindow"}])
        active = refreshed[-1].get("data") or {}
        if active.get("app_name") != "Google Chrome":
            continue
        screenshot, ocr_text, page_hint = claude_browser_snapshot(active)
        if page_hint == "claude_live":
            active = dict(active)
            active["page_hint"] = page_hint
            active["screenshot_path"] = screenshot.get("path")
            active["page_text_excerpt"] = ocr_text[:3000]
            return True, active
    return False, active


def claude_state_payload(active: dict | None = None, *, screenshot_path: str | None = None, ocr_text: str = "", page_hint_override: str | None = None) -> dict:
    active = active or {}
    title = active.get("window_title") or chrome_tab_value(
        'tell application "Google Chrome" to get title of active tab of front window'
    )
    url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    )
    app_name = active.get("app_name") or "Google Chrome"
    page_hint = (
        page_hint_override
        or active.get("page_hint")
        or ("claude_live" if url and "claude.ai/" in url else infer_page_hint(url, title, ocr_text))
    )
    extension_url = bool(url and url.startswith(f"chrome-extension://{CLAUDE_EXTENSION_ID}/"))
    return {
        "success": app_name == "Google Chrome"
        and bool((url and "claude.ai/" in url) or extension_url or page_hint in {"claude_live", "claude_extension_panel", "claude_extension"}),
        "data": {
            "app_name": app_name,
            "window_title": title,
            "url": url,
            "page_hint": page_hint,
            "screenshot_path": screenshot_path,
            "page_text_excerpt": ocr_text[:3000],
            "recent_issue_titles": [],
            "login_required": False,
            "active_window": active,
            "extension_name": "Claude",
            "extension_id": CLAUDE_EXTENSION_ID,
            "target_url": None if extension_url or page_hint in {"claude_extension_panel", "claude_extension"} else CLAUDE_LIVE_URL,
        },
    }


def find_claude_compose_point(screenshot_path: str) -> tuple[float, float] | None:
    target_phrases = (
        "how can i help you today",
        "message claude",
        "talk to claude",
        "ask claude",
        "send a message",
        "type / for commands",
    )
    for line in ocr_image_lines(screenshot_path):
        text = str(line.get("text") or "").strip().lower()
        if not text:
            continue
        if not any(phrase in text for phrase in target_phrases):
            continue
        x = float(line.get("x", 0.0)) + (float(line.get("w", 0.0)) * 0.5)
        y = float(line.get("y", 0.0)) + (float(line.get("h", 0.0)) * 0.5)
        return (x, y)
    return None


def find_ocr_line(lines: list[dict], *phrases: str) -> dict | None:
    lowered = tuple(phrase.lower() for phrase in phrases)
    for line in lines:
        text = str(line.get("text") or "").strip().lower()
        if text and any(phrase in text for phrase in lowered):
            return line
    return None


def ocr_line_screen_center(line: dict, screenshot: dict) -> tuple[float, float] | None:
    screenshot_path = str(screenshot.get("path") or "")
    if not screenshot_path:
        return None
    point = (
        float(line.get("x", 0.0)) + (float(line.get("w", 0.0)) * 0.5),
        float(line.get("y", 0.0)) + (float(line.get("h", 0.0)) * 0.5),
    )
    screen = screen_size_data()
    return normalize_image_point(
        point,
        screenshot_path,
        float(screen.get("width") or 0.0),
        float(screen.get("height") or 0.0),
    )


def claude_sidepanel_points(screenshot: dict) -> tuple[tuple[float, float], tuple[float, float]]:
    screenshot_path = str(screenshot.get("path") or "")
    width = float(screenshot.get("width") or 0.0)
    height = float(screenshot.get("height") or 0.0)
    screen = screen_size_data()
    screen_width = float(screen.get("width") or width or 1.0)
    screen_height = float(screen.get("height") or height or 1.0)
    scale_x = screen_width / width if width else 1.0
    scale_y = screen_height / height if height else 1.0
    lines = ocr_image_lines(screenshot_path) if screenshot_path else []

    compose_line = find_ocr_line(
        lines,
        "type / for commands",
        "how can i help you today",
        "message claude",
        "talk to claude",
    )
    controls_line = find_ocr_line(lines, "act without asking")
    plus_line = None
    arrow_line = None
    if controls_line:
        for line in lines:
            raw = str(line.get("text") or "").strip()
            if "→" in raw and abs(float(line.get("y", 0.0)) - float(controls_line.get("y", 0.0))) <= 80.0:
                arrow_line = line
                continue
            if raw != "+":
                continue
            if float(line.get("x", 0.0)) <= float(controls_line.get("x", 0.0)) + 400.0:
                continue
            if abs(float(line.get("y", 0.0)) - float(controls_line.get("y", 0.0))) > 80.0:
                continue
            plus_line = line
            break

    if compose_line:
        compose_point = (
            float(compose_line.get("x", 0.0)) + (float(compose_line.get("w", 0.0)) * 0.5),
            float(compose_line.get("y", 0.0)) + (float(compose_line.get("h", 0.0)) * 0.5),
        )
    elif controls_line:
        compose_point = (
            max(width * 0.78, float(controls_line.get("x", 0.0)) + 280.0),
            max(80.0, float(controls_line.get("y", 0.0)) - 55.0),
        )
    else:
        compose_point = (max(100.0, width * 0.83), max(100.0, height - 215.0))

    if arrow_line:
        send_point = (
            float(arrow_line.get("x", 0.0)) + (float(arrow_line.get("w", 0.0)) * 0.5),
            float(arrow_line.get("y", 0.0)) + (float(arrow_line.get("h", 0.0)) * 0.5),
        )
    elif plus_line:
        send_point = (
            float(plus_line.get("x", 0.0)) + float(plus_line.get("w", 0.0)) + 140.0,
            float(plus_line.get("y", 0.0)) + (float(plus_line.get("h", 0.0)) * 0.5),
        )
    elif controls_line:
        send_point = (
            max(80.0, width - 42.0),
            float(controls_line.get("y", 0.0)) + (float(controls_line.get("h", 0.0)) * 0.5),
        )
    else:
        send_point = (max(80.0, width - 42.0), max(80.0, height - 170.0))

    return (
        (compose_point[0] * scale_x, compose_point[1] * scale_y),
        (send_point[0] * scale_x, send_point[1] * scale_y),
    )


def attached_panel_compose_point(screenshot: dict) -> tuple[float, float]:
    width = float(screenshot.get("width") or 0.0)
    height = float(screenshot.get("height") or 0.0)
    screen = screen_size_data()
    screen_width = float(screen.get("width") or width or 1.0)
    screen_height = float(screen.get("height") or height or 1.0)
    scale_x = screen_width / width if width else 1.0
    scale_y = screen_height / height if height else 1.0
    x = width * 0.86 if width else screen_width * 0.86
    y = height * 0.80 if height else screen_height * 0.80
    return (x * scale_x, y * scale_y)


def attached_panel_send_point(screenshot: dict) -> tuple[float, float] | None:
    screenshot_path = str(screenshot.get("path") or "")
    width = float(screenshot.get("width") or 0.0)
    height = float(screenshot.get("height") or 0.0)
    if not screenshot_path or not width or not height:
        return None
    try:
        from PIL import Image
    except Exception:
        return None

    image = Image.open(screenshot_path).convert("RGB")
    min_x = int(width * 0.78)
    min_y = int(height * 0.70)
    max_y = int(height * 0.97)
    visited = set()
    best = None

    def is_orange(pixel: tuple[int, int, int]) -> bool:
        r, g, b = pixel
        return r > 170 and 80 < g < 190 and b < 140 and (r - g) > 20

    for y in range(min_y, max_y):
        for x in range(min_x, int(width)):
            if (x, y) in visited:
                continue
            if not is_orange(image.getpixel((x, y))):
                continue
            stack = [(x, y)]
            visited.add((x, y))
            pixels = []
            while stack:
                cx, cy = stack.pop()
                pixels.append((cx, cy))
                for nx, ny in ((cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)):
                    if nx < min_x or nx >= int(width) or ny < min_y or ny >= max_y:
                        continue
                    if (nx, ny) in visited:
                        continue
                    visited.add((nx, ny))
                    if is_orange(image.getpixel((nx, ny))):
                        stack.append((nx, ny))
            if len(pixels) < 80:
                continue
            xs = [p[0] for p in pixels]
            ys = [p[1] for p in pixels]
            bbox = (min(xs), min(ys), max(xs), max(ys))
            area = len(pixels)
            center = ((bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0)
            if best is None or area > best["area"]:
                best = {"area": area, "center": center, "bbox": bbox}

    if best is None:
        return None

    screen = screen_size_data()
    screen_width = float(screen.get("width") or width or 1.0)
    screen_height = float(screen.get("height") or height or 1.0)
    scale_x = screen_width / width if width else 1.0
    scale_y = screen_height / height if height else 1.0
    return (best["center"][0] * scale_x, best["center"][1] * scale_y)


def claude_attached_panel_type_and_send(text: str, screenshot: dict) -> dict:
    if not ensure_frontmost_app("Google Chrome"):
        return {
            "success": False,
            "error": "Google Chrome is not frontmost before attached-panel input.",
            "typed_visible": False,
        }

    panel_before_ocr = claude_panel_ocr_text(screenshot)
    typed_result = claude_ax_prompt(text)
    if not typed_result.get("success"):
        compose_point = attached_panel_compose_point(screenshot)
        responses = run_bridge(
            [
                {"action": "Click", "x": compose_point[0], "y": compose_point[1], "button": "left", "double": False},
                {"action": "Type", "text": text},
            ]
        )
        typed_result = {
            "success": all(response.get("success") for response in responses),
            "compose_point": compose_point,
            "responses": responses,
        }

    time.sleep(0.5)
    typed_shot = screenshot_data()
    typed_ocr = claude_panel_ocr_text(typed_shot)
    normalized_text = " ".join(text.lower().split())
    typed_visible = normalized_text[:48] in " ".join(typed_ocr.lower().split())

    ax_submit_attempted = bool(typed_result.get("success"))
    send_point = attached_panel_send_point(typed_shot)
    ax_send_x = typed_result.get("send_x")
    ax_send_y = typed_result.get("send_y")
    if send_point is None and ax_send_x is not None and ax_send_y is not None:
        send_point = (float(ax_send_x), float(ax_send_y))
    send_click_responses = []
    # If prompt text is still visible in the composer, force an explicit send click.
    # This avoids false positives where AX set-value succeeded but submit did not fire.
    if typed_visible and send_point and ensure_frontmost_app("Google Chrome"):
        send_click_responses = run_bridge(
            [{"action": "Click", "x": send_point[0], "y": send_point[1], "button": "left", "double": False}]
        )
        time.sleep(0.6)

    post_shot = screenshot_data()
    post_ocr = claude_panel_ocr_text(post_shot)
    prompt_still_visible = normalized_text[:48] in " ".join(post_ocr.lower().split())
    response_excerpt = extract_new_panel_response(panel_before_ocr, post_ocr, text)
    return_fallback_responses = []
    return_fallback_used = False
    if (ax_submit_attempted or typed_visible) and prompt_still_visible and ensure_frontmost_app("Google Chrome"):
        return_fallback_responses = run_bridge([{"action": "KeyPress", "key": "return", "modifiers": []}])
        return_fallback_used = True
        time.sleep(0.8)
        post_shot = screenshot_data()
        post_ocr = claude_panel_ocr_text(post_shot)
        prompt_still_visible = normalized_text[:48] in " ".join(post_ocr.lower().split())
        response_excerpt = extract_new_panel_response(panel_before_ocr, post_ocr, text)

    response_started = bool(response_excerpt)
    if not response_started and ensure_frontmost_app("Google Chrome"):
        for _ in range(8):
            time.sleep(1.0)
            poll_shot = screenshot_data()
            poll_ocr = claude_panel_ocr_text(poll_shot)
            candidate = extract_new_panel_response(panel_before_ocr, poll_ocr, text)
            if candidate:
                post_shot = poll_shot
                post_ocr = poll_ocr
                response_excerpt = candidate
                response_started = True
                break

    send_completed = bool(ax_submit_attempted or typed_visible) and (
        not prompt_still_visible or response_started
    )

    return {
        "success": send_completed,
        "ax_submit_attempted": ax_submit_attempted,
        "typed_visible": typed_visible,
        "prompt_still_visible": prompt_still_visible,
        "response_started": response_started,
        "claude_response_excerpt": response_excerpt,
        "send_point": send_point,
        "send_click_responses": send_click_responses,
        "return_fallback_used": return_fallback_used,
        "return_fallback_responses": return_fallback_responses,
        "panel_before_excerpt": panel_before_ocr[:1500],
        "typed_screenshot_path": typed_shot.get("path"),
        "typed_ocr_excerpt": typed_ocr[:1500],
        "post_screenshot_path": post_shot.get("path"),
        "post_ocr_excerpt": post_ocr[:1500],
        "ax_result": typed_result,
    }


def claude_type_and_send(text: str, screenshot: dict) -> dict:
    if not ensure_frontmost_app("Google Chrome"):
        return {
            "success": False,
            "error": "Google Chrome is not frontmost before typing.",
            "responses": [],
            "typed_visible": False,
        }
    compose_point, send_point = claude_sidepanel_points(screenshot)
    responses = run_bridge(
        [
            {"action": "Click", "x": compose_point[0], "y": compose_point[1], "button": "left", "double": False},
            {"action": "Type", "text": text},
        ]
    )
    time.sleep(0.6)
    typed_screenshot = screenshot_data()
    typed_ocr_text = ocr_image(str(typed_screenshot.get("path") or ""))
    normalized_text = " ".join(text.lower().split())
    typed_visible = normalized_text[:48] in " ".join(typed_ocr_text.lower().split())
    retry_used = False
    if not typed_visible:
        retry_used = True
        retry_compose_point, retry_send_point = claude_sidepanel_points(typed_screenshot)
        compose_point = retry_compose_point
        send_point = retry_send_point
        retry_responses = run_bridge(
            [
                {"action": "Click", "x": compose_point[0], "y": compose_point[1], "button": "left", "double": False},
                {"action": "Type", "text": text},
            ]
        )
        responses.extend(retry_responses)
        time.sleep(0.6)
        typed_screenshot = screenshot_data()
        typed_ocr_text = ocr_image(str(typed_screenshot.get("path") or ""))
        typed_visible = normalized_text[:48] in " ".join(typed_ocr_text.lower().split())
    send_responses = []
    if typed_visible:
        if not ensure_frontmost_app("Google Chrome"):
            return {
                "success": False,
                "error": "Google Chrome lost focus before sending.",
                "compose_point": compose_point,
                "send_point": send_point,
                "responses": responses,
                "typed_visible": typed_visible,
                "retry_used": retry_used,
                "typed_screenshot_path": typed_screenshot.get("path"),
                "typed_ocr_excerpt": typed_ocr_text[:1500],
            }
        send_responses = run_bridge(
            [
                {"action": "Click", "x": send_point[0], "y": send_point[1], "button": "left", "double": False},
            ]
        )
        responses.extend(send_responses)
    return {
        "success": all(response.get("success") for response in responses) and typed_visible and bool(send_responses),
        "compose_point": compose_point,
        "send_point": send_point,
        "responses": responses,
        "typed_visible": typed_visible,
        "retry_used": retry_used,
        "typed_screenshot_path": typed_screenshot.get("path"),
        "typed_ocr_excerpt": typed_ocr_text[:1500],
    }


def sidepanel_hint_visible(page_hint: str, ocr_text: str) -> bool:
    haystack = (ocr_text or "").lower()
    return page_hint in {"claude_extension_panel", "claude_extension"} or any(
        phrase in haystack
        for phrase in (
            "how can i help you today",
            "type / for commands",
            "act without asking",
            "close side panel",
            "toggle quick mode",
            "sonnet 4.6",
        )
    )


def claude_stop_active_panel_if_needed(screenshot: dict, ocr_text: str) -> dict:
    lower = (ocr_text or "").lower()
    if "stop claude" not in lower and "started debugging this browser" not in lower:
        return {"stopped": False, "ready": True}
    lines = ocr_image_lines(str(screenshot.get("path") or ""))
    stop_line = find_ocr_line(lines, "stop claude")
    if stop_line is None:
        return {"stopped": False, "ready": False, "error": "Claude panel is busy and Stop Claude button was not found."}
    stop_point = ocr_line_screen_center(stop_line, screenshot)
    if stop_point is None:
        return {"stopped": False, "ready": False, "error": "Could not resolve Stop Claude button position."}
    if not ensure_frontmost_app("Google Chrome"):
        return {"stopped": False, "ready": False, "error": "Google Chrome lost focus before stopping Claude."}
    responses = run_bridge(
        [{"action": "Click", "x": stop_point[0], "y": stop_point[1], "button": "left", "double": False}]
    )
    time.sleep(1.2)
    post_shot = screenshot_data()
    post_ocr = ocr_image(str(post_shot.get("path") or ""))
    ready = any(
        phrase in post_ocr.lower()
        for phrase in (
            "how can i help you today",
            "type / for commands",
            "act without asking",
            "reply to claude",
        )
    ) and "stop claude" not in post_ocr.lower()
    return {
        "stopped": True,
        "ready": ready,
        "responses": responses,
        "post_screenshot_path": post_shot.get("path"),
        "post_ocr_excerpt": post_ocr[:1500],
    }


def claude_failure_payload(
    *,
    text: str,
    phase: str,
    reason: str,
    active: dict | None = None,
    current_url: str = "",
    attempt_count: int = 0,
    attached: bool = False,
    response_started: bool = False,
    extra_data: dict | None = None,
) -> dict:
    data = {
        "active_window": active or {},
        "current_url": current_url,
        "typed_text": text,
        "failure_phase": phase,
        "failure_reason": reason,
        "attempt_count": attempt_count,
        "response_started": response_started,
        "claude.attached": attached,
    }
    if extra_data:
        data.update(extra_data)
    return {
        "success": False,
        "error": f"{phase}: {reason}",
        "failure_phase": phase,
        "failure_reason": reason,
        "response_started": response_started,
        "attempt_count": attempt_count,
        "data": data,
    }


def claude_prompt_live(text: str) -> dict:
    wants_sentry = "sentry" in text.lower()
    attempt_count = 0
    preflight = desktop_preflight()
    if not preflight.get("ok"):
        return claude_failure_payload(
            text=text,
            phase="preflight",
            reason=str(preflight.get("failure_reason") or "Desktop preflight failed."),
            active=preflight.get("active_window") or {},
            current_url="",
            attempt_count=attempt_count,
            attached=False,
            response_started=False,
            extra_data={"preflight": preflight},
        )

    if wants_sentry:
        attempt_count += 1
        if not chrome_set_front_tab_url(SENTRY_URL):
            active = (run_bridge([{"action": "GetActiveWindow"}])[-1].get("data") or {})
            current_url = chrome_tab_value(
                'tell application "Google Chrome" to get URL of active tab of front window'
            ) or ""
            return claude_failure_payload(
                text=text,
                phase="focus_sentry_tab",
                reason="Failed to focus the Sentry issues tab in Chrome.",
                active=active,
                current_url=current_url,
                attempt_count=attempt_count,
            )
        time.sleep(1.2)

    current_url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    ) or ""
    active = (run_bridge([{"action": "GetActiveWindow"}])[-1].get("data") or {})
    if active.get("app_name") != "Google Chrome":
        activate_app("Google Chrome")
        time.sleep(0.35)
        active = (run_bridge([{"action": "GetActiveWindow"}])[-1].get("data") or {})
    if active.get("app_name") != "Google Chrome":
        return claude_failure_payload(
            text=text,
            phase="focus_sentry_tab" if wants_sentry else "focus_browser",
            reason="Google Chrome is not frontmost.",
            active=active,
            current_url=current_url,
            attempt_count=attempt_count,
        )

    if wants_sentry and not url_matches_target(current_url, SENTRY_URL):
        return claude_failure_payload(
            text=text,
            phase="focus_sentry_tab",
            reason="Failed to keep Sentry in the active Chrome tab.",
            active=active,
            current_url=current_url,
            attempt_count=attempt_count,
            extra_data={"expected_url": SENTRY_URL},
        )

    sentry_context = None
    if wants_sentry and "sentry.io" in current_url:
        sentry_shot, sentry_ocr_text, sentry_page_hint = claude_browser_snapshot(active)
        sentry_titles = extract_issue_titles(sentry_ocr_text)
        sentry_context_parts = [
            f"Current Sentry URL: {current_url}",
            f"Page hint: {sentry_page_hint}",
        ]
        if sentry_titles:
            sentry_context_parts.append("Visible issue titles: " + "; ".join(sentry_titles[:8]))
        sentry_context = "\n".join(part for part in sentry_context_parts if part)
        text = (
            "Analyze the current Sentry issues page in this tab. "
            "Summarize the visible unresolved issues. "
            "If there are no unresolved issues, say exactly: No unresolved issues match the current Sentry filter."
        )

    attempt_count += 1
    if wants_sentry:
        ok, active = chrome_open_claude_sidepanel_on_current_tab()
    else:
        ok, active = chrome_open_claude_extension_panel()
    if not ok:
        return claude_failure_payload(
            text=text,
            phase=str(active.get("failure_phase") or "attach_sidepanel"),
            reason=str(
                active.get("failure_reason")
                or active.get("error")
                or "Claude side panel did not open."
            ),
            active=active,
            current_url=str(active.get("current_url") or current_url),
            attempt_count=int(active.get("attempt_count") or attempt_count),
            attached=bool(active.get("claude_attached")),
            extra_data={"sentry_context": sentry_context},
        )

    live_url = chrome_tab_value(
        'tell application "Google Chrome" to get URL of active tab of front window'
    ) or ""
    attached = bool(active.get("claude_attached"))
    if live_url.startswith(f"chrome-extension://{CLAUDE_EXTENSION_ID}/"):
        return claude_failure_payload(
            text=text,
            phase="attach_sidepanel",
            reason="Claude side panel detached into a standalone extension tab.",
            active=active,
            current_url=live_url,
            attempt_count=int(active.get("attempt_count") or attempt_count),
            attached=False,
            extra_data={"sentry_context": sentry_context},
        )
    if wants_sentry and not url_matches_target(live_url, SENTRY_URL):
        return claude_failure_payload(
            text=text,
            phase="attach_sidepanel",
            reason="Claude side panel opened on the wrong tab.",
            active=active,
            current_url=live_url,
            attempt_count=int(active.get("attempt_count") or attempt_count),
            attached=attached,
            extra_data={"expected_url": SENTRY_URL, "sentry_context": sentry_context},
        )
    if wants_sentry and not attached:
        return claude_failure_payload(
            text=text,
            phase="attach_sidepanel",
            reason="Claude side panel is not attached to the Sentry tab.",
            active=active,
            current_url=live_url,
            attempt_count=int(active.get("attempt_count") or attempt_count),
            attached=False,
            extra_data={"sentry_context": sentry_context},
        )

    screenshot, ocr_text, page_hint = claude_browser_snapshot(active)
    stop_result = claude_stop_active_panel_if_needed(screenshot, ocr_text)
    if stop_result.get("stopped"):
        screenshot = screenshot_data()
        ocr_text = ocr_image(str(screenshot.get("path") or ""))
    if stop_result.get("ready") is False:
        return claude_failure_payload(
            text=text,
            phase="ready_panel",
            reason=str(
                stop_result.get("error")
                or "Claude side panel is busy and did not return to an idle composer."
            ),
            active=active,
            current_url=live_url,
            attempt_count=int(active.get("attempt_count") or attempt_count),
            attached=attached,
            extra_data={"stop_result": stop_result, "sentry_context": sentry_context},
        )

    attempt_count += 1
    if wants_sentry:
        typed_result = claude_attached_panel_type_and_send(text, screenshot)
    else:
        typed_result = claude_type_and_send(text, screenshot)
    if not bool(typed_result.get("success")):
        return claude_failure_payload(
            text=text,
            phase="submit",
            reason="Failed typing or sending the Claude prompt through the side panel.",
            active=active,
            current_url=live_url,
            attempt_count=attempt_count,
            attached=attached,
            response_started=bool(typed_result.get("response_started")),
            extra_data={"type_result": typed_result, "sentry_context": sentry_context},
        )

    post_screenshot, post_ocr_text, post_page_hint = claude_browser_snapshot(active)
    normalized_text = " ".join(text.lower().split())
    prompt_cleared = normalized_text[:48] not in " ".join(post_ocr_text.lower().split())
    response_started = bool(typed_result.get("response_started"))
    if not response_started:
        lower_post = post_ocr_text.lower()
        response_started = any(
            token in lower_post
            for token in (
                "thinking",
                "analyzing",
                "stop claude",
                "continue",
                "reply...",
                "copy",
                "retry",
            )
        ) and prompt_cleared
    if not prompt_cleared:
        return claude_failure_payload(
            text=text,
            phase="submit",
            reason="Prompt is still visible in the composer after submit.",
            active=active,
            current_url=live_url,
            attempt_count=attempt_count,
            attached=attached,
            response_started=response_started,
            extra_data={
                "post_screenshot_path": post_screenshot.get("path"),
                "post_ocr_excerpt": post_ocr_text[:1500],
                "type_result": typed_result,
                "sentry_context": sentry_context,
            },
        )
    if not response_started:
        return claude_failure_payload(
            text=text,
            phase="response_wait",
            reason="Claude did not start responding after submit.",
            active=active,
            current_url=live_url,
            attempt_count=attempt_count,
            attached=attached,
            response_started=False,
            extra_data={
                "post_screenshot_path": post_screenshot.get("path"),
                "post_ocr_excerpt": post_ocr_text[:1500],
                "type_result": typed_result,
                "sentry_context": sentry_context,
            },
        )

    payload = claude_state_payload(
        active,
        screenshot_path=str(post_screenshot.get("path") or screenshot.get("path") or ""),
        ocr_text=post_ocr_text or ocr_text,
        page_hint_override=post_page_hint or page_hint,
    )
    payload["success"] = True
    payload["failure_phase"] = None
    payload["failure_reason"] = None
    payload["response_started"] = True
    payload["attempt_count"] = attempt_count
    payload["data"]["active_window"] = active
    payload["data"]["typed_text"] = text
    payload["data"]["sentry_context"] = sentry_context
    payload["data"]["sent"] = True
    payload["data"]["conversation_started"] = True
    payload["data"]["prompt_cleared"] = True
    payload["data"]["type_result"] = typed_result
    payload["data"]["claude_response_excerpt"] = typed_result.get("claude_response_excerpt")
    payload["data"]["failure_phase"] = None
    payload["data"]["failure_reason"] = None
    payload["data"]["attempt_count"] = attempt_count
    payload["data"]["response_started"] = True
    payload["data"]["claude.attached"] = attached
    return payload


def main() -> int:
    if len(sys.argv) < 2:
        print(
            "usage: desktop_control.py <launch|type|calc-input|open-url|keypress|move|click|scroll|active-window|screenshot|screen-record|screen-size|wait|sequence-json|sequence-file> [args...]",
            file=sys.stderr,
        )
        return 2

    cmd = sys.argv[1]
    if cmd == "launch":
        if len(sys.argv) < 3:
            print("usage: desktop_control.py launch <App Name>", file=sys.stderr)
            return 2
        app_name = " ".join(sys.argv[2:])
        responses = run_bridge([{"action": "LaunchApp", "app_name": app_name}])
        activate_app(app_name)
        time.sleep(0.4)
        responses.extend(run_bridge([{"action": "GetActiveWindow"}]))
        active = responses[-1].get("data") or {}
        success = responses[0].get("success") and active.get("app_name") == app_name
        print(json.dumps({"success": success, "data": active}))
        return 0 if success else 1

    if cmd == "type":
        if len(sys.argv) < 3:
            print("usage: desktop_control.py type <text>", file=sys.stderr)
            return 2
        responses = run_bridge([{"action": "Type", "text": " ".join(sys.argv[2:])}])
        print(json.dumps(responses[0]))
        return 0

    if cmd == "calc-input":
        if len(sys.argv) < 3:
            print("usage: desktop_control.py calc-input <expression>", file=sys.stderr)
            return 2
        expr = " ".join(sys.argv[2:]).replace(" ", "")
        quit_app("Calculator")
        time.sleep(0.3)
        responses = run_bridge([{"action": "LaunchApp", "app_name": "Calculator"}])
        activate_app("Calculator")
        time.sleep(0.4)
        responses.extend(
            run_bridge(
                [
                    {"action": "GetActiveWindow"},
                    {"action": "KeyPress", "key": "escape", "modifiers": []},
                    {"action": "KeyPress", "key": "escape", "modifiers": []},
                ]
            )
        )
        if not all(resp.get("success") for resp in responses):
            print(json.dumps({"success": False, "steps": responses}))
            return 1
        active = responses[1].get("data") or {}
        if active.get("app_name") != "Calculator":
            print(
                json.dumps(
                    {
                        "success": False,
                        "error": "Calculator did not become frontmost",
                        "steps": responses,
                    }
                )
            )
            return 1
        x0, y0, _, _ = find_calculator_window_bounds()
        actions = []
        clear_x, clear_y = CALC_BUTTONS["AC"]
        for _ in range(2):
            actions.append({"action": "MouseMove", "x": x0 + clear_x, "y": y0 + clear_y})
            actions.append(
                {
                    "action": "Click",
                    "x": x0 + clear_x,
                    "y": y0 + clear_y,
                    "button": "left",
                    "double": False,
                }
            )
            actions.append({"action": "KeyPress", "key": "escape", "modifiers": []})
        for ch in expr:
            mapped = CALC_BUTTONS.get(ch)
            if mapped is None:
                raise SystemExit(f"unsupported calculator char: {ch}")
            x = x0 + mapped[0]
            y = y0 + mapped[1]
            actions.append({"action": "MouseMove", "x": x, "y": y})
            actions.append({"action": "Click", "x": x, "y": y, "button": "left", "double": False})
        responses = run_bridge(actions)
        ok = all(resp.get("success") for resp in responses)
        print(json.dumps({"success": ok, "data": {"expression": expr}, "steps": responses}))
        return 0 if ok else 1

    if cmd == "open-url":
        if len(sys.argv) != 3:
            print("usage: desktop_control.py open-url <url>", file=sys.stderr)
            return 2
        url = sys.argv[2]
        ok, active = chrome_open_url_live(url)
        print(
            json.dumps(
                {
                    "success": ok,
                    "data": {
                        "url": url,
                        "active_window": active,
                    },
                }
            )
        )
        return 0 if ok else 1

    if cmd == "chrome-open-sentry":
        preflight = desktop_preflight()
        if not preflight.get("ok"):
            payload = claude_failure_payload(
                text="",
                phase="preflight",
                reason=str(preflight.get("failure_reason") or "Desktop preflight failed."),
                active=preflight.get("active_window") or {},
                attempt_count=0,
                extra_data={"preflight": preflight},
            )
            print(json.dumps(payload))
            print(payload.get("error", "preflight failed"), file=sys.stderr)
            return 1
        ok, active = chrome_open_url_live(SENTRY_URL)
        payload = browser_state_payload(include_probe=True, probe_url=SENTRY_URL)
        payload["success"] = (
            payload.get("success")
            and ok
            and active.get("app_name") == "Google Chrome"
        )
        payload["data"]["active_window"] = active
        print(json.dumps(payload))
        if not payload["success"]:
            print(payload.get("error", "Failed opening Sentry."), file=sys.stderr)
        return 0 if payload["success"] else 1

    if cmd == "chrome-open-claude-extension":
        preflight = desktop_preflight()
        if not preflight.get("ok"):
            payload = claude_failure_payload(
                text="",
                phase="preflight",
                reason=str(preflight.get("failure_reason") or "Desktop preflight failed."),
                active=preflight.get("active_window") or {},
                attempt_count=0,
                extra_data={"preflight": preflight},
            )
            print(json.dumps(payload))
            print(payload.get("error", "preflight failed"), file=sys.stderr)
            return 1
        ok, active = chrome_open_claude_extension_panel()
        payload = claude_state_payload(
            active,
            screenshot_path=active.get("screenshot_path"),
            ocr_text=active.get("page_text_excerpt") or "",
            page_hint_override=active.get("page_hint"),
        )
        payload["success"] = (
            payload.get("success")
            and ok
            and active.get("app_name") == "Google Chrome"
        )
        payload["data"]["active_window"] = active
        payload["data"]["extension_name"] = "Claude"
        payload["data"]["extension_id"] = CLAUDE_EXTENSION_ID
        payload["data"]["target_url"] = None
        print(json.dumps(payload))
        if not payload["success"]:
            print(payload.get("error", "Failed opening Claude extension."), file=sys.stderr)
        return 0 if payload["success"] else 1

    if cmd == "claude-prompt":
        if len(sys.argv) < 3:
            print("usage: desktop_control.py claude-prompt <text>", file=sys.stderr)
            return 2
        payload = claude_prompt_live(" ".join(sys.argv[2:]))
        print(json.dumps(payload))
        if not payload.get("success"):
            print(payload.get("error", "Failed sending Claude prompt."), file=sys.stderr)
        return 0 if payload.get("success") else 1

    if cmd == "browser-state":
        payload = browser_state_payload()
        print(json.dumps(payload))
        return 0 if payload["success"] else 1

    if cmd == "keypress":
        if len(sys.argv) < 3:
            print("usage: desktop_control.py keypress <key> [modifiers...]", file=sys.stderr)
            return 2
        responses = run_bridge(
            [{"action": "KeyPress", "key": sys.argv[2], "modifiers": sys.argv[3:]}]
        )
        print(json.dumps(responses[0]))
        return 0

    if cmd == "move":
        if len(sys.argv) != 4:
            print("usage: desktop_control.py move <x> <y>", file=sys.stderr)
            return 2
        responses = run_bridge(
            [{"action": "MouseMove", "x": float(sys.argv[2]), "y": float(sys.argv[3])}]
        )
        print(json.dumps(responses[0]))
        return 0

    if cmd == "click":
        if len(sys.argv) < 4:
            print("usage: desktop_control.py click <x> <y> [left|right|double]", file=sys.stderr)
            return 2
        x = float(sys.argv[2])
        y = float(sys.argv[3])
        mode = sys.argv[4] if len(sys.argv) > 4 else "left"
        button = "right" if mode == "right" else "left"
        double = mode == "double"
        responses = run_bridge(
            [{"action": "Click", "x": x, "y": y, "button": button, "double": double}]
        )
        print(json.dumps(responses[0]))
        return 0

    if cmd == "scroll":
        if len(sys.argv) != 4:
            print("usage: desktop_control.py scroll <dx> <dy>", file=sys.stderr)
            return 2
        responses = run_bridge(
            [{"action": "Scroll", "dx": int(sys.argv[2]), "dy": int(sys.argv[3])}]
        )
        print(json.dumps(responses[0]))
        return 0

    if cmd == "active-window":
        responses = run_bridge([{"action": "GetActiveWindow"}])
        print(json.dumps(responses[0]))
        return 0

    if cmd == "screenshot":
        payload = screenshot_data()
        print(json.dumps(payload))
        return 0 if payload.get("success") else 1

    if cmd == "screen-record":
        if len(sys.argv) != 3:
            print("usage: desktop_control.py screen-record <seconds>", file=sys.stderr)
            return 2
        try:
            seconds = float(sys.argv[2])
        except ValueError:
            print("screen-record seconds must be numeric", file=sys.stderr)
            return 2
        payload = screen_record_data(seconds)
        print(json.dumps(payload))
        return 0 if payload.get("success") else 1

    if cmd == "screen-size":
        responses = run_bridge([{"action": "GetScreenSize"}])
        print(json.dumps(responses[0]))
        return 0

    if cmd == "wait":
        if len(sys.argv) != 3:
            print("usage: desktop_control.py wait <seconds>", file=sys.stderr)
            return 2
        seconds = float(sys.argv[2])
        time.sleep(seconds)
        print(json.dumps({"success": True, "data": {"waited_secs": seconds}}))
        return 0

    if cmd == "sequence-json":
        if len(sys.argv) < 3:
            print("usage: desktop_control.py sequence-json '<json array>'", file=sys.stderr)
            return 2
        actions = json.loads(" ".join(sys.argv[2:]))
        if not isinstance(actions, list):
            raise SystemExit("sequence-json expects a JSON array")
        responses = run_bridge(actions)
        ok = all(resp.get("success") for resp in responses)
        print(json.dumps({"success": ok, "steps": responses}))
        return 0

    if cmd == "sequence-file":
        if len(sys.argv) != 3:
            print("usage: desktop_control.py sequence-file <path>", file=sys.stderr)
            return 2
        actions = json.loads(Path(sys.argv[2]).read_text())
        if not isinstance(actions, list):
            raise SystemExit("sequence-file expects a JSON array file")
        responses = run_bridge(actions)
        ok = all(resp.get("success") for resp in responses)
        print(json.dumps({"success": ok, "steps": responses}))
        return 0

    print(f"unknown command: {cmd}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
