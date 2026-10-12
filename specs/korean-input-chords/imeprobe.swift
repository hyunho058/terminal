import AppKit
import Carbon

func src(_ id: String) -> TISInputSource? {
    let list = TISCreateInputSourceList(nil, true).takeRetainedValue() as! [TISInputSource]
    return list.first { s in
        let p = TISGetInputSourceProperty(s, kTISPropertyInputSourceID)!
        return (Unmanaged<CFString>.fromOpaque(p).takeUnretainedValue() as String) == id
    }
}

class V: NSView, NSTextInputClient {
    var marked = NSMutableAttributedString()
    override var acceptsFirstResponder: Bool { true }
    override func keyDown(with e: NSEvent) {
        print("keyDown chars=\(e.characters ?? "nil") ignoring=\(e.charactersIgnoringModifiers ?? "nil") flags=\(e.modifierFlags.intersection(.deviceIndependentFlagsMask).rawValue) src=\(inputContext!.selectedKeyboardInputSource ?? "?")")
        inputContext!.handleEvent(e)   // same as interpretKeyEvents for one event
        print("  after: hasMarked=\(hasMarkedText())")
    }
    override func flagsChanged(with e: NSEvent) { print("flagsChanged \(e.modifierFlags.rawValue)") }
    func insertText(_ s: Any, replacementRange: NSRange) { print("  insertText \(s) hadMarked=\(hasMarkedText())"); marked = NSMutableAttributedString() }
    override func doCommand(by sel: Selector) { print("  doCommandBySelector \(sel)") }
    func setMarkedText(_ s: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let str = (s as? NSAttributedString)?.string ?? (s as? String) ?? "?"
        print("  setMarkedText '\(str)'"); marked = NSMutableAttributedString(string: str)
    }
    func unmarkText() { print("  unmarkText"); marked = NSMutableAttributedString() }
    func selectedRange() -> NSRange { NSRange(location: 0, length: 0) }
    func markedRange() -> NSRange { marked.length == 0 ? NSRange(location: NSNotFound, length: 0) : NSRange(location: 0, length: marked.length) }
    func hasMarkedText() -> Bool { marked.length > 0 }
    func attributedSubstring(forProposedRange r: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? { nil }
    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }
    func firstRect(forCharacterRange r: NSRange, actualRange: NSRangePointer?) -> NSRect { window!.convertToScreen(NSRect(x: 10, y: 10, width: 1, height: 20)) }
    func characterIndex(for p: NSPoint) -> Int { 0 }
}

func post(_ kc: CGKeyCode, _ flags: CGEventFlags = []) {
    for down in [true, false] {
        let ev = CGEvent(keyboardEventSource: nil, virtualKey: kc, keyDown: down)!
        ev.flags = flags
        NSApp.postEvent(NSEvent(cgEvent: ev)!, atStart: false)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let win = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 300, height: 100), styleMask: [.titled], backing: .buffered, defer: false)
let v = V(frame: win.contentView!.bounds)
win.contentView = v
win.makeFirstResponder(v)
win.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
let original = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
let ko = src("com.apple.inputmethod.Korean.2SetKorean")!

var step = 0
let steps: [() -> Void] = [
    { print("== ABC: Cmd+T"); post(17, .maskCommand) },
    { print("== select Korean"); TISSelectInputSource(ko) },
    { print("== Korean, idle: Cmd+T"); post(17, .maskCommand) },
    { print("== Korean, idle: Cmd+C"); post(8, .maskCommand) },
    { print("== Korean: type h (ㅎ)"); post(4) },
    { print("== Korean, composing: Cmd+T"); post(17, .maskCommand) },
    { print("== Korean, after that: Cmd+T again"); post(17, .maskCommand) },
    { print("== Korean: Cmd+Shift+M"); post(46, [.maskCommand, .maskShift]) },
    { print("== Korean: Ctrl+C"); post(8, .maskControl) },
    { print("== restore"); TISSelectInputSource(original) },
    { app.terminate(nil) },
]
Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { _ in
    if step < steps.count { steps[step](); step += 1 }
}
app.run()
