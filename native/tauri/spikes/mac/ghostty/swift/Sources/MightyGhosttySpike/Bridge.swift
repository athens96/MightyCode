import AppKit
import Foundation
import GhosttyTerminal

/// What the user typed (as Ghostty encoded it), handed to Rust, which owns the PTY.
public typealias MGInput = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Void
/// The grid the view settled on, handed to Rust for TIOCSWINSZ.
public typealias MGResize = @convention(c) (UnsafeMutableRawPointer?, UInt16, UInt16) -> Void

/// One terminal: the vendored view over an in-memory session (Swift's `AgentTerminalHost` shape), its controller with Swift's settings.
@MainActor
final class SpikeTerminal {
    let controller: TerminalController
    let session: InMemoryTerminalSession
    let view: AppTerminalView

    init(frame: NSRect, context: UnsafeMutableRawPointer?, input: @escaping MGInput, resize: @escaping MGResize) {
        let theme = TerminalTheme(
            light: TerminalConfiguration().background("FAF9F6"),
            dark: TerminalConfiguration().background("202020").foreground("DEDCD8").cursorColor("E1AB91")
        )
        controller = TerminalController(theme: theme) { settings in
            settings.withFontSize(14)
            settings.withCustom("clipboard-read", "deny")
            settings.withCustom("clipboard-write", "ask")
        }
        let address = UInt(bitPattern: context)
        session = InMemoryTerminalSession(
            write: { data in
                data.withUnsafeBytes { raw in
                    input(UnsafeMutableRawPointer(bitPattern: address), raw.bindMemory(to: UInt8.self).baseAddress, raw.count)
                }
            },
            resize: { viewport in
                resize(UnsafeMutableRawPointer(bitPattern: address), viewport.columns, viewport.rows)
            }
        )
        view = AppTerminalView(frame: frame)
        view.configuration = TerminalSurfaceOptions(backend: .inMemory(session), fontSize: 14)
        view.controller = controller
    }
}

private var terminals: [UnsafeMutableRawPointer: SpikeTerminal] = [:]

@_cdecl("mg_spike_open")
public func mg_spike_open(_ parent: UnsafeMutableRawPointer, _ x: Double, _ y: Double, _ width: Double, _ height: Double,
                          _ context: UnsafeMutableRawPointer?, _ input: MGInput, _ resize: MGResize) -> UnsafeMutableRawPointer? {
    MainActor.assumeIsolated {
        let host = Unmanaged<NSView>.fromOpaque(parent).takeUnretainedValue()
        let terminal = SpikeTerminal(frame: NSRect(x: x, y: y, width: width, height: height), context: context, input: input, resize: resize)
        host.addSubview(terminal.view)
        let handle = Unmanaged.passUnretained(terminal.view).toOpaque()
        terminals[handle] = terminal
        return handle
    }
}

@_cdecl("mg_spike_view")
public func mg_spike_view(_ handle: UnsafeMutableRawPointer) -> UnsafeMutableRawPointer? {
    MainActor.assumeIsolated { terminals[handle].map { Unmanaged.passUnretained($0.view).toOpaque() } }
}

/// PTY output into the session (any thread: the session queues it).
@_cdecl("mg_spike_write")
public func mg_spike_write(_ handle: UnsafeMutableRawPointer, _ bytes: UnsafePointer<UInt8>, _ length: Int) {
    let data = Data(bytes: bytes, count: length)
    DispatchQueue.main.async {
        MainActor.assumeIsolated { terminals[handle]?.session.receive(data) }
    }
}

@_cdecl("mg_spike_type")
public func mg_spike_type(_ handle: UnsafeMutableRawPointer, _ text: UnsafePointer<CChar>) {
    let line = String(cString: text)
    MainActor.assumeIsolated { _ = terminals[handle]?.view.paste(text: line) }
}

@_cdecl("mg_spike_enter")
public func mg_spike_enter(_ handle: UnsafeMutableRawPointer) -> Bool {
    MainActor.assumeIsolated { terminals[handle]?.view.sendKey(.enter) ?? false }
}

/// The viewport as UTF-8 into `buffer`; its length, or -1 without a surface.
@_cdecl("mg_spike_read_text")
public func mg_spike_read_text(_ handle: UnsafeMutableRawPointer, _ buffer: UnsafeMutablePointer<UInt8>, _ capacity: Int) -> Int {
    MainActor.assumeIsolated {
        guard let text = terminals[handle]?.session.readViewportText() else { return -1 }
        let bytes = Array(text.utf8.prefix(capacity))
        bytes.withUnsafeBufferPointer { buffer.update(from: $0.baseAddress!, count: bytes.count) }
        return bytes.count
    }
}

@_cdecl("mg_spike_free")
public func mg_spike_free(_ handle: UnsafeMutableRawPointer) {
    MainActor.assumeIsolated {
        guard let terminal = terminals.removeValue(forKey: handle) else { return }
        terminal.view.controller = nil
        terminal.view.removeFromSuperview()
    }
}
