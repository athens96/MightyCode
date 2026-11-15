// Quits a running app the way the Mac rehearsal will (plan §3 0b-4): NSRunningApplication.terminate(), the polite quit that takes the
// app's install-on-quit path, with no Apple Events script (whose Automation consent a runner could not give). Prints one JSON line.
//
//   swiftc -O terminate.swift -o terminate && ./terminate <pid> <seconds to wait>
import AppKit

let arguments = CommandLine.arguments
guard arguments.count == 3, let pid = pid_t(arguments[1]), let limit = Double(arguments[2]) else {
    print(#"{"error":"usage: terminate <pid> <seconds>"}"#)
    exit(2)
}
guard let app = NSRunningApplication(processIdentifier: pid) else {
    print(#"{"error":"no running application with that pid"}"#)
    exit(1)
}
let sent = app.terminate()
let start = Date()
while !app.isTerminated && Date().timeIntervalSince(start) < limit {
    RunLoop.current.run(until: Date().addingTimeInterval(0.25))
}
let seconds = Date().timeIntervalSince(start)
print("{\"bundleIdentifier\":\"\(app.bundleIdentifier ?? "")\",\"terminateSent\":\(sent),\"terminated\":\(app.isTerminated),\"seconds\":\(String(format: "%.1f", seconds))}")
exit(app.isTerminated ? 0 : 1)
