#!/usr/bin/env python3
"""OpenFang Desktop Bridge — macOS desktop control over JSON-line stdio protocol.

Reads JSON commands from stdin (one per line), executes desktop actions via
PyObjC/Quartz, and writes JSON responses to stdout (one per line).

Usage:
    python desktop_bridge.py [--timeout 30] [--scale 1.0] [--display 0]
"""

import argparse
import base64
import json
import sys
import time
import traceback


def respond(obj):
    """Write a JSON response to stdout."""
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    parser = argparse.ArgumentParser(description="OpenFang Desktop Bridge")
    parser.add_argument("--timeout", type=int, default=30, help="Per-action timeout in seconds")
    parser.add_argument("--scale", type=float, default=1.0, help="Screenshot scale factor")
    parser.add_argument("--display", type=int, default=0, help="Display index (0 = main)")
    args = parser.parse_args()

    # Check for required dependencies
    try:
        import Quartz
        from Quartz import (
            CGEventCreateMouseEvent,
            CGEventPost,
            CGEventCreateKeyboardEvent,
            CGEventKeyboardSetUnicodeString,
            CGEventCreateScrollWheelEvent,
            CGWindowListCreateImage,
            CGRectNull,
            CGMainDisplayID,
            CGDisplayBounds,
            kCGEventMouseMoved,
            kCGEventLeftMouseDown,
            kCGEventLeftMouseUp,
            kCGEventRightMouseDown,
            kCGEventRightMouseUp,
            kCGHIDEventTap,
            kCGWindowListOptionOnScreenOnly,
            kCGNullWindowID,
            kCGEventScrollWheel,
            kCGScrollEventUnitLine,
            kCGEventKeyDown,
            kCGEventKeyUp,
            kCGEventFlagMaskShift,
            kCGEventFlagMaskControl,
            kCGEventFlagMaskAlternate,
            kCGEventFlagMaskCommand,
        )
        from Quartz import CGEventSetIntegerValueField, kCGKeyboardEventKeycode
    except ImportError:
        respond({
            "success": False,
            "error": "PyObjC Quartz not installed. Run: pip3 install pyobjc-framework-Quartz pyobjc-framework-Cocoa"
        })
        return

    try:
        import Cocoa
        from Cocoa import NSWorkspace, NSBitmapImageRep, NSPNGFileType
        try:
            from AppKit import NSApplicationActivateIgnoringOtherApps
        except Exception:
            NSApplicationActivateIgnoringOtherApps = 1
    except ImportError:
        respond({
            "success": False,
            "error": "PyObjC Cocoa not installed. Run: pip3 install pyobjc-framework-Cocoa"
        })
        return

    # Test screenshot permission on init, but do not block non-screenshot actions.
    screenshot_error = None
    try:
        test_img = CGWindowListCreateImage(
            CGRectNull,
            kCGWindowListOptionOnScreenOnly,
            kCGNullWindowID,
            0
        )
        if test_img is None:
            screenshot_error = (
                "Screen Recording permission required. Grant permission in "
                "System Preferences > Privacy & Security > Screen Recording."
            )
    except Exception as e:
        screenshot_error = (
            f"Screenshot permission test failed: {e}. Grant Screen Recording permission in "
            "System Preferences."
        )

    # Key name → virtual keycode mapping (macOS)
    KEY_MAP = {
        "return": 36, "enter": 36, "tab": 48, "space": 49, "delete": 51,
        "backspace": 51, "escape": 53, "esc": 53,
        "left": 123, "right": 124, "down": 125, "up": 126,
        "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
        "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111,
        "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
        "a": 0, "b": 11, "c": 8, "d": 2, "e": 14, "f": 3, "g": 5, "h": 4,
        "i": 34, "j": 38, "k": 40, "l": 37, "m": 46, "n": 45, "o": 31,
        "p": 35, "q": 12, "r": 15, "s": 1, "t": 17, "u": 32, "v": 9,
        "w": 13, "x": 7, "y": 16, "z": 6,
        "0": 29, "1": 18, "2": 19, "3": 20, "4": 21, "5": 23, "6": 22,
        "7": 26, "8": 28, "9": 25,
        "-": 27, "=": 24, "[": 33, "]": 30, "\\": 42, ";": 41, "'": 39,
        ",": 43, ".": 47, "/": 44, "`": 50,
    }

    MODIFIER_MAP = {
        "shift": kCGEventFlagMaskShift,
        "control": kCGEventFlagMaskControl, "ctrl": kCGEventFlagMaskControl,
        "alt": kCGEventFlagMaskAlternate, "option": kCGEventFlagMaskAlternate,
        "command": kCGEventFlagMaskCommand, "cmd": kCGEventFlagMaskCommand,
        "meta": kCGEventFlagMaskCommand, "super": kCGEventFlagMaskCommand,
    }

    # Signal ready
    respond(
        {
            "success": True,
            "data": {
                "status": "ready",
                "platform": "macos",
                "screen_recording_available": screenshot_error is None,
                "screen_recording_error": screenshot_error,
            },
        }
    )

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        action = None
        try:
            cmd = json.loads(line)
            action = cmd.get("action", "")
            result = handle_command(
                cmd,
                action,
                args,
                Quartz,
                Cocoa,
                KEY_MAP,
                MODIFIER_MAP,
                screenshot_error,
            )
            respond(result)
        except Exception as e:
            respond({"success": False, "error": f"{type(e).__name__}: {e}"})

        if action == "Close":
            break


def handle_command(cmd, action, args, Quartz, Cocoa, KEY_MAP, MODIFIER_MAP, screenshot_error):
    if action == "Screenshot":
        return do_screenshot(args, Quartz, Cocoa, screenshot_error)

    elif action == "MouseMove":
        x = cmd.get("x", 0)
        y = cmd.get("y", 0)
        point = Quartz.CGPointMake(float(x), float(y))
        event = Quartz.CGEventCreateMouseEvent(None, Quartz.kCGEventMouseMoved, point, 0)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, event)
        return {"success": True, "data": {"moved_to": {"x": x, "y": y}}}

    elif action == "Click":
        x = cmd.get("x", 0)
        y = cmd.get("y", 0)
        button = cmd.get("button", "left")
        double = cmd.get("double", False)
        point = Quartz.CGPointMake(float(x), float(y))

        if button == "right":
            down_type = Quartz.kCGEventRightMouseDown
            up_type = Quartz.kCGEventRightMouseUp
        else:
            down_type = Quartz.kCGEventLeftMouseDown
            up_type = Quartz.kCGEventLeftMouseUp

        # Move mouse first
        move_event = Quartz.CGEventCreateMouseEvent(None, Quartz.kCGEventMouseMoved, point, 0)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, move_event)
        time.sleep(0.05)

        # Click
        click_count = 2 if double else 1
        for i in range(click_count):
            down_event = Quartz.CGEventCreateMouseEvent(None, down_type, point, 0)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, down_event)
            time.sleep(0.05)
            up_event = Quartz.CGEventCreateMouseEvent(None, up_type, point, 0)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, up_event)
            if i < click_count - 1:
                time.sleep(0.05)

        return {"success": True, "data": {"clicked": {"x": x, "y": y, "button": button, "double": double}}}

    elif action == "Type":
        text = cmd.get("text", "")
        if not text:
            return {"success": False, "error": "Missing 'text' parameter"}

        def press_unicode(char):
            down_event = Quartz.CGEventCreateKeyboardEvent(None, 0, True)
            Quartz.CGEventKeyboardSetUnicodeString(down_event, len(char), char)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, down_event)
            time.sleep(0.01)
            up_event = Quartz.CGEventCreateKeyboardEvent(None, 0, False)
            Quartz.CGEventKeyboardSetUnicodeString(up_event, len(char), char)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, up_event)
            time.sleep(0.01)

        def press_key(key_name):
            keycode = KEY_MAP.get(key_name)
            if keycode is None:
                raise ValueError(f"Unknown key for typing: {key_name!r}")
            down_event = Quartz.CGEventCreateKeyboardEvent(None, keycode, True)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, down_event)
            time.sleep(0.01)
            up_event = Quartz.CGEventCreateKeyboardEvent(None, keycode, False)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, up_event)
            time.sleep(0.01)

        start = time.time()
        try:
            for char in text:
                if time.time() - start > args.timeout:
                    return {"success": False, "error": f"Type command timed out after {args.timeout}s"}
                if char == "\n":
                    press_key("return")
                elif char == "\t":
                    press_key("tab")
                else:
                    press_unicode(char)
        except Exception as e:
            return {"success": False, "error": f"Quartz typing failed: {e}"}

        return {"success": True, "data": {"typed": text}}

    elif action == "KeyPress":
        key = cmd.get("key", "").lower()
        modifiers = [m.lower() for m in cmd.get("modifiers", [])]

        keycode = KEY_MAP.get(key)
        if keycode is None:
            return {"success": False, "error": f"Unknown key: '{key}'. Available: {', '.join(sorted(KEY_MAP.keys()))}"}

        # Build modifier flags
        flags = 0
        for mod in modifiers:
            flag = MODIFIER_MAP.get(mod)
            if flag is None:
                return {"success": False, "error": f"Unknown modifier: '{mod}'. Available: {', '.join(sorted(MODIFIER_MAP.keys()))}"}
            flags |= flag

        # Key down
        down = Quartz.CGEventCreateKeyboardEvent(None, keycode, True)
        if flags:
            Quartz.CGEventSetFlags(down, flags)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, down)
        time.sleep(0.05)

        # Key up
        up = Quartz.CGEventCreateKeyboardEvent(None, keycode, False)
        if flags:
            Quartz.CGEventSetFlags(up, flags)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, up)

        return {"success": True, "data": {"pressed": {"key": key, "modifiers": modifiers}}}

    elif action == "GetActiveWindow":
        ws = Cocoa.NSWorkspace.sharedWorkspace()
        active_app = ws.activeApplication()
        if active_app is None:
            return {"success": True, "data": {"app_name": None, "window_title": None, "pid": None, "bundle_id": None}}

        app_name = active_app.get("NSApplicationName", "")
        pid = active_app.get("NSApplicationProcessIdentifier", 0)
        bundle_id = active_app.get("NSApplicationBundleIdentifier", "")

        # Get window title via Quartz
        window_title = ""
        try:
            window_list = Quartz.CGWindowListCopyWindowInfo(
                Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements,
                Quartz.kCGNullWindowID
            )
            for window in window_list:
                if window.get("kCGWindowOwnerPID") == pid:
                    title = window.get("kCGWindowName", "")
                    if title:
                        window_title = title
                        break
        except Exception:
            pass

        return {"success": True, "data": {
            "app_name": app_name,
            "window_title": window_title,
            "pid": pid,
            "bundle_id": bundle_id,
        }}

    elif action == "LaunchApp":
        app_name = cmd.get("app_name", "")
        if not app_name:
            return {"success": False, "error": "Missing 'app_name' parameter"}
        ws = Cocoa.NSWorkspace.sharedWorkspace()
        success = ws.launchApplication_(app_name)
        if not success:
            return {"success": False, "error": f"Failed to launch application: {app_name}"}

        # Give the app time to launch, then force it frontmost.
        time.sleep(1)
        try:
            running = ws.runningApplications()
            for app in running:
                if app.localizedName() == app_name:
                    app.activateWithOptions_(NSApplicationActivateIgnoringOtherApps)
                    time.sleep(0.5)
                    break
        except Exception:
            pass

        return {"success": True, "data": {"launched": app_name}}

    elif action == "Scroll":
        dx = cmd.get("dx", 0)
        dy = cmd.get("dy", -3)  # Default: scroll down 3 lines
        event = Quartz.CGEventCreateScrollWheelEvent(
            None,
            Quartz.kCGScrollEventUnitLine,
            2,  # number of axes
            int(dy),
            int(dx),
        )
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, event)
        return {"success": True, "data": {"scrolled": {"dx": dx, "dy": dy}}}

    elif action == "GetScreenSize":
        display_id = Quartz.CGMainDisplayID()
        bounds = Quartz.CGDisplayBounds(display_id)
        width = int(bounds.size.width)
        height = int(bounds.size.height)
        return {"success": True, "data": {"width": width, "height": height}}

    elif action == "Close":
        return {"success": True, "data": {"status": "closed"}}

    else:
        return {"success": False, "error": f"Unknown action: {action}"}


def do_screenshot(args, Quartz, Cocoa, screenshot_error):
    """Capture a screenshot of the entire screen as base64 PNG."""
    if screenshot_error is not None:
        return {"success": False, "error": screenshot_error}
    image = Quartz.CGWindowListCreateImage(
        Quartz.CGRectNull,
        Quartz.kCGWindowListOptionOnScreenOnly,
        Quartz.kCGNullWindowID,
        0
    )
    if image is None:
        return {"success": False, "error": "Screenshot capture failed. Check Screen Recording permission."}

    width = Quartz.CGImageGetWidth(image)
    height = Quartz.CGImageGetHeight(image)

    # Convert CGImage → NSBitmapImageRep → PNG data
    bitmap = Cocoa.NSBitmapImageRep.alloc().initWithCGImage_(image)
    png_data = bitmap.representationUsingType_properties_(Cocoa.NSPNGFileType, None)

    if png_data is None:
        return {"success": False, "error": "Failed to encode screenshot as PNG"}

    b64 = base64.b64encode(bytes(png_data)).decode("utf-8")
    return {
        "success": True,
        "data": {
            "image_base64": b64,
            "format": "png",
            "width": width,
            "height": height,
        }
    }


if __name__ == "__main__":
    main()
