// Measures what a Cmd chord produces under 2-Set Korean, idle AND mid-composition, and
// replays winit 0.30.13's keyDown state machine (view.rs:442-498) to say whether the app
// would receive a KeyboardInput at all. Events go through NSTextInputContext.handleEvent
// in a real NSApp, so the input method actually sees them (a bare CGEvent does not).
import AppKit
import Carbon

func src(_ id: String) -> TISInputSource? {
    let list = TISCreateInputSourceList(nil, true).takeRetainedValue() as! [TISInputSource]
    return list.first { s in
        let p = TISGetInputSourceProperty(s, kTISPropertyInputSourceID)!
        return (Unmanaged<CFString>.fromOpaque(p).takeUnretainedValue() as String) == id
    }
}
// winit's get_modifierless_char (event.rs:24-77): UCKeyTranslate, modifiers 0, on the
// current keyboard-LAYOUT input source. This is what `key_without_modifiers` reports.
func modifierless(_ kc: UInt16) -> String {
    let s = TISCopyCurrentKeyboardLayoutInputSource().takeRetainedValue()
    let data = Unmanaged<CFData>.fromOpaque(TISGetInputSourceProperty(s, kTISPropertyUnicodeKeyLayoutData)!).takeUnretainedValue()
    let layout = CFDataGetBytePtr(data)!.withMemoryRebound(to: UCKeyboardLayout.self, capacity: 1) { $0 }
    var dead: UInt32 = 0; var len = 0; var buf = [UniChar](repeating: 0, count: 16)
    UCKeyTranslate(layout, kc, UInt16(kUCKeyActionDisplay), 0, UInt32(LMGetKbdType()), OptionBits(kUCKeyTranslateNoDeadKeysMask), &dead, 16, &len, &buf)
    return String(utf16CodeUnits: buf, count: len)
}
func layoutID() -> String {
    let s = TISCopyCurrentKeyboardLayoutInputSource().takeRetainedValue()
    let p = TISGetInputSourceProperty(s, kTISPropertyInputSourceID)!
    return Unmanaged<CFString>.fromOpaque(p).takeUnretainedValue() as String
}

enum St { case ground, preedit, committed }

class V: NSView, NSTextInputClient {
    var marked = ""
    var st = St.ground
    var forward = false
    override var acceptsFirstResponder: Bool { true }
    override func keyDown(with e: NSEvent) {
        let old = st
        forward = false
        let f = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        print("keyDown kc=\(e.keyCode) chars=\(e.characters ?? "nil") ignoring=\(e.charactersIgnoringModifiers ?? "nil") cmd=\(f.contains(.command)) shift=\(f.contains(.shift)) layout=\(layoutID()) key_without_modifiers=\(modifierless(e.keyCode)) marked-before='\(marked)'")
        inputContext!.handleEvent(e)
        if st == .committed { marked = "" }
        let had: Bool
        switch st {
        case .committed: st = .ground; had = true
        case .preedit: had = true
        case .ground: had = old != st
        }
        let emit = !had || forward
        print("  -> winit KeyboardInput emitted: \(emit)  (state now \(st), marked-after='\(marked)')")
    }
    func insertText(_ s: Any, replacementRange: NSRange) {
        let str = (s as? NSAttributedString)?.string ?? (s as? String) ?? "?"
        let isCtl = str.unicodeScalars.first.map { CharacterSet.controlCharacters.contains($0) } ?? false
        let ce = NSApp.currentEvent
        let ceDesc = ce.map { $0.type == .keyDown ? "keyDown kc=\($0.keyCode) chars=\($0.characters ?? "nil") cmd=\($0.modifierFlags.contains(.command))" : "type=\($0.type.rawValue)" } ?? "nil"
        print("  insertText '\(str)' hadMarked=\(hasMarkedText()) currentEvent=[\(ceDesc)]")
        if hasMarkedText() && !isCtl { print("  [winit: Ime::Commit('\(str)')]"); st = .committed }
        marked = ""
    }
    override func doCommand(by sel: Selector) {
        print("  doCommandBySelector \(sel)")
        if st == .committed { return }
        forward = true
        if hasMarkedText() && st == .preedit { st = .ground }
    }
    func setMarkedText(_ s: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let str = (s as? NSAttributedString)?.string ?? (s as? String) ?? "?"
        print("  setMarkedText '\(str)'")
        marked = str
        st = str.isEmpty ? .ground : .preedit
    }
    func unmarkText() { print("  unmarkText"); marked = "" }
    func selectedRange() -> NSRange { NSRange(location: 0, length: 0) }
    func markedRange() -> NSRange { marked.isEmpty ? NSRange(location: NSNotFound, length: 0) : NSRange(location: 0, length: (marked as NSString).length) }
    func hasMarkedText() -> Bool { !marked.isEmpty }
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
let abc = src("com.apple.keylayout.ABC")!
let ko = src("com.apple.inputmethod.Korean.2SetKorean")!
let C = CGEventFlags.maskCommand, S = CGEventFlags.maskShift

var step = 0
// optprobe: what `characters` is under Cmd+Opt (+Shift), ABC and 2-Set Korean, idle.
let O = CGEventFlags.maskAlternate
let steps: [() -> Void] = [
    { print("== select ABC"); TISSelectInputSource(abc) },
    { print("== ABC: Cmd+Opt+S"); post(1, [C, O]) },
    { print("== ABC: Cmd+Opt+Shift+M"); post(46, [C, O, S]) },
    { print("== ABC: Cmd+Opt+C"); post(8, [C, O]) },
    { print("== ABC: Cmd+Opt+1"); post(18, [C, O]) },
    { print("== select Korean"); TISSelectInputSource(ko) },
    { print("== KO: Cmd+Opt+S"); post(1, [C, O]) },
    { print("== KO: Cmd+Opt+Shift+M"); post(46, [C, O, S]) },
    { print("== KO: Cmd+Opt+C"); post(8, [C, O]) },
    { print("== KO: Cmd+Opt+1"); post(18, [C, O]) },
    { print("== restore"); TISSelectInputSource(original) },
    { app.terminate(nil) },
]
Timer.scheduledTimer(withTimeInterval: 0.8, repeats: true) { _ in
    if step < steps.count { steps[step](); step += 1 }
}
app.run()
