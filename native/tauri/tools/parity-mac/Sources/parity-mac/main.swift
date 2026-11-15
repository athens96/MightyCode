// parity-mac: Swift's own verdicts from MightyCore, as JSON on stdout (mac-plan.md §3 Step 0e).
//
//   parity-mac stage <zip> [<expected bundle id>]   AppUpdateService.stage + validate on a package (copied to a temporary folder
//                                                   first, so nothing is written beside it): {"verdict":"ok", …} or {"verdict":
//                                                   "refused","error":…}. Owner M-I (every package's check).
//   parity-mac install-script <cases.json>          AppUpdateService.installScript for each case {name, stagedApp, destination,
//                                                   replacing?, runningExecutable?, pid, relaunch}: [{name, script}]. Owner M-H.
//   parity-mac manifest <file> <public key base64>  AppUpdateManifest.parse with the key: the fields Swift read, or its refusal.
//                                                   Owner M-H.
//   parity-mac state-corpus <out folder>            M-B: StateRepository-encoded snapshots with every Mac-only field. Not written yet.
//   parity-mac pairing write|read <folder>          M-B: the four pairing files by Swift's own code. Not written yet.
//   parity-mac defaults-keys                        M-B: the UserDefaults keys and value types the app reads. Not written yet.
//
// Exit 0 with a verdict (an "ok" and a refusal alike), 2 for a mode not written yet or a wrong call, 1 when the tool itself failed.
import Foundation
import MightyCore

func emit(_ value: Any) {
    let data = (try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .withoutEscapingSlashes])) ?? Data("{}".utf8)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

func fail(_ message: String, code: Int32) -> Never {
    emit(["error": message])
    exit(code)
}

func describe(_ error: Error) -> String {
    (error as? LocalizedError)?.errorDescription ?? String(describing: error)
}

func asset(_ asset: AppUpdateAsset?) -> Any {
    guard let asset else { return NSNull() }
    return ["url": asset.url.absoluteString, "sha256": asset.sha256 ?? NSNull(), "size": asset.size ?? NSNull()] as [String: Any]
}

let arguments = Array(CommandLine.arguments.dropFirst())
guard let mode = arguments.first else { fail("usage: parity-mac <mode> …", code: 2) }

switch mode {
case "stage":
    guard arguments.count >= 2 else { fail("usage: parity-mac stage <zip> [<expected bundle id>]", code: 2) }
    let expected = arguments.count >= 3 ? arguments[2] : "dev.mightycode.native"
    let work = FileManager.default.temporaryDirectory.appendingPathComponent("parity-mac-stage-\(UUID().uuidString)", isDirectory: true)
    defer { try? FileManager.default.removeItem(at: work) }
    do {
        try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
        let package = work.appendingPathComponent(AppUpdateService.packageName)
        try FileManager.default.copyItem(at: URL(fileURLWithPath: arguments[1]), to: package)
        let service = AppUpdateService(directory: work)
        let app = try await service.stage(package: package, expectedBundleIdentifier: expected)
        let info = NSDictionary(contentsOf: app.appendingPathComponent("Contents/Info.plist")) as? [String: Any] ?? [:]
        emit(["verdict": "ok", "app": app.lastPathComponent, "bundleIdentifier": info["CFBundleIdentifier"] ?? NSNull(),
              "executable": info["CFBundleExecutable"] ?? NSNull(), "shortVersion": info["CFBundleShortVersionString"] ?? NSNull(),
              "bundleVersion": info["CFBundleVersion"] ?? NSNull()])
    } catch {
        emit(["verdict": "refused", "error": describe(error)])
    }

case "install-script":
    guard arguments.count == 2, let data = FileManager.default.contents(atPath: arguments[1]),
          let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any], let cases = root["cases"] as? [[String: Any]] else {
        fail("usage: parity-mac install-script <cases.json> ({\"cases\": [...]})", code: 2)
    }
    var scripts: [[String: Any]] = []
    for item in cases {
        guard let name = item["name"] as? String, let staged = item["stagedApp"] as? String, let destination = item["destination"] as? String,
              let pid = item["pid"] as? Int else { fail("a case needs name, stagedApp, destination and pid", code: 2) }
        let replacing = (item["replacing"] as? String).map { URL(fileURLWithPath: $0) }
        let script = AppUpdateService.installScript(
            stagedApp: URL(fileURLWithPath: staged), destination: URL(fileURLWithPath: destination), replacing: replacing,
            runningExecutable: item["runningExecutable"] as? String ?? "MightyCode", pid: Int32(pid), relaunch: item["relaunch"] as? Bool ?? true)
        scripts.append(["name": name, "script": script])
    }
    emit(["cases": scripts])

case "manifest":
    guard arguments.count == 3, let data = FileManager.default.contents(atPath: arguments[1]) else {
        fail("usage: parity-mac manifest <file> <public key base64>", code: 2)
    }
    let key = Data(base64Encoded: arguments[2])
    do {
        let manifest = try AppUpdateManifest.parse(data, publicKey: key)
        var windows: [String: Any] = [:]
        for (architecture, value) in manifest.windows { windows[architecture] = asset(value) }
        emit(["verdict": "ok", "version": manifest.version, "build": manifest.build ?? NSNull(), "notes": manifest.notes ?? NSNull(),
              "publishedAt": manifest.publishedAt ?? NSNull(), "minimumSystemVersion": manifest.minimumSystemVersion ?? NSNull(),
              "macos": asset(manifest.macos), "windows": windows, "signed": manifest.signed])
    } catch {
        emit(["verdict": "refused", "error": describe(error)])
    }

case "state-corpus", "pairing", "defaults-keys":
    emit(["error": "not written yet", "mode": mode, "owner": "M-B"])
    exit(2)

default:
    fail("unknown mode \(mode)", code: 2)
}
