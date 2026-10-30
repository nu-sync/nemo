import AppKit
import Foundation

let eventPath = CommandLine.arguments[1]
FileManager.default.createFile(atPath: eventPath, contents: nil)
let events = FileHandle(forWritingAtPath: eventPath)!

final class PasteView: NSTextView {
    override func paste(_ sender: Any?) {
        string = ""
        super.paste(sender)
        let event: [String: Any] = [
            "event": "paste_received",
            "unix_seconds": Date().timeIntervalSince1970,
            "text": string
        ]
        if let data = try? JSONSerialization.data(withJSONObject: event) {
            events.write(data)
            events.write(Data([10]))
            events.synchronizeFile()
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let menu = NSMenu()
let appItem = NSMenuItem()
let appMenu = NSMenu()
appMenu.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
appItem.submenu = appMenu
menu.addItem(appItem)
let editItem = NSMenuItem()
let editMenu = NSMenu(title: "Edit")
editMenu.addItem(withTitle: "Paste", action: #selector(NSTextView.paste(_:)), keyEquivalent: "v")
editItem.submenu = editMenu
menu.addItem(editItem)
app.mainMenu = menu
let window = NSWindow(contentRect: NSRect(x: 200, y: 250, width: 700, height: 260),
                      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
window.title = "Nemo Delivery Check"
let text = PasteView(frame: window.contentView!.bounds)
text.autoresizingMask = [.width, .height]
text.font = NSFont.systemFont(ofSize: 22)
text.isRichText = false
text.textContainerInset = NSSize(width: 18, height: 18)
window.contentView!.addSubview(text)
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(text)
app.activate(ignoringOtherApps: true)
app.run()
