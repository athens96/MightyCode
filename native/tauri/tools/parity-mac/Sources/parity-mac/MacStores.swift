// M-B's modes of parity-mac: the state round trip, the pairing files and the defaults keys, each by MightyCore's own code
// (mac-plan.md §3 Step 0e, M-B). JSON on stdout; the corpus and the pairing files go to the folders given.
import CryptoKit
import Foundation
import MightyCore

// MARK: - The state corpus

/// The properties no instance in the corpus sets (`Type.property`), from the decoded snapshot: what the corpus misses.
private final class Coverage {
    var seen = Set<String>(), filled = Set<String>()

    func walk(_ value: Any) {
        let mirror = Mirror(reflecting: value)
        switch mirror.displayStyle {
        case .optional:
            if let child = mirror.children.first { walk(child.value) }
        case .collection, .set:
            for child in mirror.children { walk(child.value) }
        case .dictionary:
            for child in mirror.children { for part in Mirror(reflecting: child.value).children { walk(part.value) } }
        case .struct, .class:
            let type = String(describing: Swift.type(of: value))
            for child in mirror.children {
                guard let label = child.label, !label.hasPrefix("_"), !label.hasPrefix("$") else { continue }
                let name = "\(type).\(label)"
                seen.insert(name)
                let inner = Mirror(reflecting: child.value)
                if inner.displayStyle == .optional && inner.children.isEmpty { continue }
                filled.insert(name)
                walk(child.value)
            }
        default:
            break
        }
    }

    var unset: [String] { seen.subtracting(filled).sorted() }
}

private func jsonObject(_ text: String) -> [String: Any]? {
    (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any]
}

private func decodes<T: Decodable>(_ type: T.Type, _ object: Any) -> Bool {
    guard let data = try? JSONSerialization.data(withJSONObject: object) else { return false }
    return (try? JSONDecoder().decode(type, from: data)) != nil
}

/// `object` with only the keys of `optional` that still let the whole decode as `T` (a value this tool wrote wrong is reported, not
/// fatal): the required keys, then each optional one.
private func keeping<T: Decodable>(_ type: T.Type, _ object: [String: Any], required: Set<String>, refused: inout [String], path: String) -> [String: Any] {
    var kept = object.filter { required.contains($0.key) }
    for key in object.keys.sorted() where !required.contains(key) {
        var next = kept
        next[key] = object[key]
        if decodes(type, next) { kept = next } else { refused.append("\(path).\(key)") }
    }
    return kept
}

private let stamp = "2026-11-02T03:04:05Z", fine = "2026-11-02T03:04:05.123Z", later = "2026-11-02T03:09:07.456Z"

private func log(_ id: String, _ kind: String, _ text: String, activity: Bool = false, image: Bool = false) -> [String: Any] {
    var entry: [String: Any] = ["id": id, "kind": kind, "text": text, "timestamp": stamp, "provider": "claude"]
    if activity {
        entry["activity"] = ["id": id + "-act", "provider": "claude", "kind": "tool", "state": "completed", "toolName": "Bash",
                             "summary": "ls -la /tmp", "output": "a\nb", "durationMs": 1250.5] as [String: Any]
    }
    if image {
        entry["images"] = [["hash": String(repeating: "ab", count: 32), "mediaType": "image/png", "width": 640, "height": 480, "bytes": 2048,
                            "source": "paste", "path": "/tmp/mighty/image.png"] as [String: Any]]
    }
    return entry
}

private let usage: [String: Any] = ["inputTokens": 1200, "outputTokens": 340, "cacheReadTokens": 50, "cacheCreationTokens": 7]

private func graphRun(_ id: String) -> [String: Any] {
    var agent: [String: Any] = ["id": "agent-1", "parentID": "agent-0", "title": "Explore", "input": "look around", "status": "completed"]
    agent["entries"] = [log("agent-entry-1", "assistant", "found it")]
    agent["kind"] = "task"; agent["usage"] = usage; agent["activityGeneration"] = 3
    let agentRecord: [String: Any] = ["responseId": "resp-2", "model": "claude-sonnet-4-5", "usage": usage, "activityIds": ["act-2"], "markedAsConfigured": true]
    agent["responseRecords"] = [agentRecord]
    agent["background"] = true; agent["outputFile"] = "/tmp/mighty/agent-1.txt"; agent["noticeReply"] = "done"; agent["sourceBlockId"] = "block-1"
    let identity: [String: String] = ["type": "Explore", "requestedModel": "sonnet", "resolvedModel": "claude-sonnet-4-5", "isolation": "worktree"]
    agent["identity"] = identity
    let figures: [String: Any] = ["startedAt": fine, "endedAt": later, "toolCalls": 4, "exitCode": 0]
    agent["figures"] = figures

    var run: [String: Any] = ["id": id, "input": "hello / 안녕 \"quoted\"", "status": "completed"]
    run["rootEntries"] = [log(id + "-root", "assistant", "root")]
    run["agents"] = [agent]
    run["resultEntries"] = [log(id + "-result", "assistant", "result")]
    run["sourceRunID"] = "run-0"; run["finalOutput"] = "done"; run["usage"] = usage
    let record: [String: Any] = ["responseId": "resp-1", "model": "claude-opus-4-5", "usage": usage, "activityIds": ["act-1"], "markedAsConfigured": false]
    run["responseRecords"] = [record]
    run["provider"] = "claude"; run["nodeModelLabel"] = "Opus"; run["configuredModel"] = "opus"; run["configuredEffort"] = "high"
    run["noticeRun"] = true; run["noticeTitle"] = "Background work"; run["configuredAdvisor"] = "opus"
    var call: [String: Any] = ["id": "call-1", "caller": "main", "startedAt": fine, "endedAt": later, "state": "completed"]
    call["redacted"] = false; call["text"] = "consider X"; call["errorCode"] = "none"; call["beforeChange"] = true; call["followed"] = true
    let advisor: [String: Any] = ["model": "opus", "usage": usage, "total": 1, "calls": [call]]
    run["advisor"] = advisor
    let step: [String: String] = ["key": "step-1", "content": "Plan", "activeForm": "Planning", "status": "completed", "startedAt": fine, "endedAt": later]
    run["steps"] = [step]
    run["stepsCarried"] = ["step-1"]
    return run
}
private func session(_ id: String, workspace: String, kind: String, provider: String, extra: [String: Any]) -> [String: Any] {
    var value: [String: Any] = [
        "id": id, "workspaceId": workspace, "title": "Pane \(id)", "kind": kind, "provider": provider, "model": "default",
        "settings": ["effort": "default", "permissionMode": "manual", "maxTurns": NSNull(), "maxBudgetUsd": NSNull()] as [String: Any],
        "status": "completed", "logs": [log(id + "-user", "user", "do the thing"), log(id + "-reply", "assistant", "done", activity: true)],
        "createdAt": stamp,
    ]
    for (key, item) in extra { value[key] = item }
    return value
}

private let sessionRequired: Set<String> = ["id", "workspaceId"]
private let workspaceRequired: Set<String> = ["id", "name", "path", "createdAt"]

/// The snapshot of one case as JSON, every field set somewhere. Built a value at a time: one literal this size is more than Swift's
/// type checker takes.
private func fullCase(local: String, worktree: String, remote: Workspace) -> [String: Any] {
    let w1 = "ws-local-1"
    var c: [String: Any] = [:]
    c["model"] = "opus"
    var settings: [String: Any] = ["effort": "high", "permissionMode": "plan", "maxTurns": 12, "maxBudgetUsd": 2.5]
    settings["fastMode"] = true; settings["webSearch"] = "live"; settings["networkAccess"] = true; settings["advisor"] = "opus"
    c["settings"] = settings
    c["logs"] = [log("pane-claude-1-user", "user", "do the thing"), log("pane-claude-1-reply", "assistant", "done", activity: true),
                 log("pane-claude-1-image", "image", "image.png", image: true)]
    c["resumeId"] = "8f14e45f-ceea-467f-a8f5-6f3c1b2d3e4f"
    let timing: [String: Any] = ["startedAt": fine, "lastObservedAt": later, "finishedAt": later, "isApproximate": true]
    c["runTiming"] = timing
    var used: [String: Any] = ["provider": "claude", "source": "stream", "tokenScope": "session", "model": "claude-opus-4-5", "selectedModel": "opus"]
    used["providerSessionId"] = "8f14e45f-ceea-467f-a8f5-6f3c1b2d3e4f"
    for (key, count) in [("inputTokens", 1200), ("outputTokens", 340), ("cacheReadTokens", 50), ("cacheWriteTokens", 7), ("reasoningTokens", 9),
                         ("totalTokens", 1606), ("contextUsedTokens", 30000), ("contextWindowTokens", 200000)] { used[key] = count }
    used["costUSD"] = 0.25; used["costScope"] = "session"
    let limit: [String: Any] = ["kind": "five_hour", "percentUsed": 22.5, "resetsAt": later]
    used["rateLimits"] = [limit]; used["rateLimitsUpdatedAt"] = fine; used["updatedAt"] = later
    c["sessionUsage"] = used
    c["agentViewMode"] = "mighty"; c["mightyStyle"] = "ouroboros"; c["mightyStyleHash"] = String(repeating: "cd", count: 32); c["mightyStyleSince"] = fine
    c["graphRuns"] = [graphRun("run-1")]
    let blockSize: [String: Double] = ["width": 420, "height": 260.5]
    c["graphBlockSizes"] = ["block-1": blockSize]
    c["graphSizesLayout"] = 1
    let resultSize: [String: Double] = ["width": 500, "height": 300], planSize: [String: Double] = ["width": 480.5, "height": 320]
    c["graphResultSize"] = resultSize; c["graphPlanSize"] = planSize
    c["graphViewMode"] = "timeline"; c["flowLogCollapsed"] = true; c["titleMode"] = "fixed"
    var plan: [String: Any] = ["id": "plan-1", "runId": "run-1", "plan": "Do X then Y", "planTruncated": true, "receivedAt": fine, "decidedAt": later]
    plan["outcome"] = "approvedConfirm"; plan["feedback"] = "fine"; plan["graphRunId"] = "run-1"; plan["launchOverride"] = "acceptEdits"
    c["planHistory"] = [plan]
    let todo: [String: String] = ["id": "todo-1", "content": "Write it", "activeForm": "Writing it", "status": "in_progress"]
    let todos: [String: Any] = ["items": [todo], "runId": "run-1"]
    c["todoProgress"] = todos
    let task: [String: String] = ["id": "bg-1", "toolUseId": "toolu-1", "kind": "bash", "description": "npm test", "startedAt": fine, "status": "completed",
                                  "summary": "ok", "endedAt": later]
    let background: [String: Any] = ["tasks": [task], "turnEnded": true]
    c["backgroundWork"] = background
    c["workingFolder"] = worktree
    var tree: [String: Any] = ["worktreePath": worktree, "branch": "mighty/pane-claude-1", "baseBranch": "main", "baseCommit": String(repeating: "a1", count: 20)]
    tree["checkout"] = local; tree["port"] = 41234; tree["setupCommand"] = "npm ci"; tree["setupStatus"] = "done"; tree["setupExitCode"] = 0
    tree["mergedCommit"] = String(repeating: "b2", count: 20); tree["preMergeCommit"] = String(repeating: "c3", count: 20)
    tree["workingFolder"] = worktree; tree["noteGiven"] = true
    c["paneWorktree"] = tree
    c["askTarget"] = "codex"
    // `ShellBlockRun.key(entryId:index:)`: the reply entry's id and the block's number.
    var run: [String: Any] = ["key": "pane-claude-1-reply#1", "command": "ls -la", "state": "completed", "exitCode": 0, "output": "a\nb"]
    run["startedAt"] = fine; run["endedAt"] = later
    c["shellRuns"] = [run]
    let claude = session("pane-claude-1", workspace: w1, kind: "claude", provider: "claude", extra: c)

    let child = session("pane-child-2", workspace: w1, kind: "claude", provider: "claude",
                        extra: ["parentSessionId": "pane-claude-1", "workingFolder": worktree, "titleMode": "auto"])
    var codexSettings: [String: Any] = ["effort": "high", "permissionMode": "manual", "webSearch": "cached"]
    codexSettings["maxTurns"] = NSNull(); codexSettings["maxBudgetUsd"] = NSNull()
    let codexTodo: [String: String] = ["content": "Check", "status": "pending"]
    let codexTodos: [String: Any] = ["items": [codexTodo]]
    let codex = session("pane-codex-5", workspace: w1, kind: "claude", provider: "codex", extra: ["settings": codexSettings, "todoProgress": codexTodos])
    let noLogs: [Any] = []
    let shell = session("pane-shell-3", workspace: w1, kind: "shell", provider: "claude", extra: ["workingFolder": worktree, "status": "idle", "logs": noLogs])
    let browser = session("pane-browser-4", workspace: w1, kind: "browser", provider: "claude",
                          extra: ["workspaceProfileKey": "profile-ws-local-1", "ownerSessionId": "pane-claude-1", "logs": noLogs, "status": "idle"])
    let remotePane = session("pane-remote-6", workspace: remote.id, kind: "claude", provider: "claude", extra: [:])

    let none: [Any] = []
    var tabs: [String: Any] = ["id": "node-tabs", "kind": "tabs", "sessionIds": ["pane-claude-1", "pane-child-2"], "selectedSessionId": "pane-claude-1"]
    tabs["ratio"] = 0.5; tabs["children"] = none
    var others: [String: Any] = ["id": "node-others", "kind": "tabs", "sessionIds": ["pane-codex-5", "pane-shell-3", "pane-browser-4"]]
    others["ratio"] = 0.5; others["children"] = none
    var root: [String: Any] = ["id": "node-root", "kind": "split", "axis": "horizontal", "ratio": 0.62]
    root["sessionIds"] = none; root["children"] = [tabs, others]

    let registered: [String: Any] = ["name": "my-model", "supportsEffort": true, "supportedEffortLevels": ["low", "high"]]
    let provider: [String: Any] = ["modeDefaults": ["plan": "opus"], "registeredModels": [registered]]
    let defaults: [String: Any] = ["claude": provider, "codex": provider]

    var keptTree: [String: Any] = ["worktreePath": worktree + "-kept", "branch": "mighty/pane-gone-9", "baseBranch": "main"]
    keptTree["baseCommit"] = String(repeating: "d4", count: 20); keptTree["checkout"] = local; keptTree["port"] = 41235
    let kept: [String: Any] = ["sessionId": "pane-gone-9", "title": "Old pane", "provider": "codex", "keptAt": stamp, "worktree": keptTree]
    var local1: [String: Any] = ["id": w1, "name": "Project 한글 / \"q\"", "path": local, "createdAt": stamp]
    local1["modelDefaults"] = defaults; local1["paneWorktreeMode"] = "every"; local1["worktreeSetupCommand"] = "npm ci"
    local1["worktreeCopyPaths"] = [".env", "config/local.json"]; local1["keptPaneWorktrees"] = [kept]
    let reference: [String: Any] = ["version": 2, "connectionId": remote.remote?.connectionId ?? "", "workspaceId": remote.remote?.workspaceId ?? "",
                                    "hostName": remote.remote?.hostName ?? ""]
    let remote1: [String: Any] = ["id": remote.id, "name": remote.name, "path": remote.path, "createdAt": remote.createdAt, "remote": reference]

    var s: [String: Any] = ["version": 1, "workspaces": [local1, remote1], "sessions": [claude, child, codex, shell, browser, remotePane]]
    s["activeWorkspaceId"] = w1; s["activeSessionId"] = "pane-claude-1"; s["layout"] = "grid"; s["theme"] = "light"; s["sidebarWidth"] = 300
    s["paneLayouts"] = [w1: root]; // Every workspace's mode, as Swift's normalize always writes them (a workspace left out takes each normalize's own default).
    s["paneLayoutModes"] = [w1: "custom", remote.id: "tabs"]; s["paneLayoutActiveSessionIds"] = [w1: "pane-claude-1"]
    s["autoUpdateCLIs"] = false; s["autoUpdatePlugins"] = false; s["expandedWorkspaceIds"] = [w1, remote.id]
    let mobile: [String: Any] = ["enabled": true, "relayURL": "wss://relay.example.com", "allowLegacyPhones": false, "directEnabled": false, "directPort": 43140]
    s["mobileRemote"] = mobile
    s["modelDefaults"] = defaults
    var phases: [String: String] = ["claudeMain": "opus", "claudeOpusAlias": "claude-opus-4-5", "claudeSonnetAlias": "claude-sonnet-4-5"]
    phases["claudeHaikuAlias"] = "claude-haiku-4-5"; phases["claudeSubagentDefault"] = "sonnet"; phases["codexReviewModel"] = "gpt-5"
    phases["codexSubagentDefault"] = "gpt-5-mini"; phases["codexPlanModeReasoningEffort"] = "high"; phases["claudeMainEffort"] = "high"
    phases["codexMainEffort"] = "medium"; phases["codexSubagentEffort"] = "low"
    s["phaseModels"] = phases
    s["sidebarCollapsed"] = true; s["autoLoginCLIs"] = false; s["backgroundWorkOpen"] = true; s["lastCreateLocation"] = "/Users/me/Projects/새 폴더"
    s["antigravityMigrated"] = true
    return s
}
/// Text that every writer escapes differently: non-ASCII, emoji, quotes, slashes, controls.
private func textCase(local: String) -> [String: Any] {
    let w = "ws-text-1"
    let odd = "탭\t줄\n\"따옴표\" back\\slash /slash/ <tag> & 'apos' + plus `tick` 😀 \u{0001}\u{001f} é"
    let pane = session("pane-text-1", workspace: w, kind: "claude", provider: "claude", extra: [
        "title": String(odd.prefix(60)), "titleMode": "fixed",
        "logs": [log("text-user", "user", odd), log("text-reply", "assistant", odd + " reply", activity: true), log("text-error", "error", "failed: /x/y")],
    ])
    return ["version": 1, "workspaces": [["id": w, "name": odd, "path": local, "createdAt": stamp] as [String: Any]], "sessions": [pane],
            "activeWorkspaceId": w, "layout": "grid", "theme": "dark", "sidebarWidth": 252.5, "lastCreateLocation": "/tmp/a\\b/c"]
}

/// Writes `<case>.saved.json` (StateRepository's own save) and `<case>.encoded.json` (JSONEncoder, no normalize) for each case.
func stateCorpus(_ out: URL) async {
    let fileManager = FileManager.default
    let work = fileManager.temporaryDirectory.appendingPathComponent("parity-mac-state-\(UUID().uuidString)", isDirectory: true)
    defer { try? fileManager.removeItem(at: work) }
    var report: [[String: Any]] = [], refused: [String] = []
    let coverage = Coverage()
    do {
        try fileManager.createDirectory(at: out, withIntermediateDirectories: true)
        let project = work.appendingPathComponent("Project 한글", isDirectory: true)
        let tree = work.appendingPathComponent("worktrees/pane-claude-1", isDirectory: true)
        for folder in [project, tree] { try fileManager.createDirectory(at: folder, withIntermediateDirectories: true) }
        // StateRepository resolves links (/var is /private/var): the corpus names the folders as it will.
        let local = project.resolvingSymlinksInPath().path, worktree = tree.resolvingSymlinksInPath().path
        for (name, build) in [("full", 0), ("text", 1)] {
            let profile = work.appendingPathComponent("profile-\(name)", isDirectory: true)
            let repository = StateRepository(directory: profile, legacyStateURL: nil)
            _ = try await repository.load()
            let remote = try await repository.approveRemoteWorkspace(connectionId: "conn-1", hostWorkspaceId: "host-ws-1", name: "Remote / 원격",
                                                                       path: "C:\\Users\\me\\remote", hostName: "Windows PC")
            var object = build == 0 ? fullCase(local: local, worktree: worktree, remote: remote) : textCase(local: local)
            // Each piece is kept only while the whole still decodes, so one wrong value of this tool's is reported, not fatal.
            if var workspaces = object["workspaces"] as? [[String: Any]] {
                for index in workspaces.indices {
                    workspaces[index] = keeping(Workspace.self, workspaces[index], required: workspaceRequired, refused: &refused, path: "\(name).workspaces[\(index)]")
                }
                object["workspaces"] = workspaces
            }
            if var sessions = object["sessions"] as? [[String: Any]] {
                for index in sessions.indices {
                    sessions[index] = keeping(RunSession.self, sessions[index], required: sessionRequired, refused: &refused, path: "\(name).sessions[\(index)]")
                }
                object["sessions"] = sessions
            }
            object = keeping(AppSnapshot.self, object, required: ["version", "workspaces", "sessions", "layout", "theme", "sidebarWidth"], refused: &refused, path: name)
            let snapshot = try JSONDecoder().decode(AppSnapshot.self, from: JSONSerialization.data(withJSONObject: object))
            coverage.walk(snapshot)
            let encoded = try JSONEncoder().encode(snapshot)
            try encoded.write(to: out.appendingPathComponent("\(name).encoded.json"))
            for workspace in snapshot.workspaces where workspace.remote == nil { _ = try await repository.approveWorkspace(workspace) }
            try await repository.save(snapshot)
            let saved = try Data(contentsOf: profile.appendingPathComponent("workspace-state.json"))
            try saved.write(to: out.appendingPathComponent("\(name).saved.json"))
            report.append(["case": name, "encodedBytes": encoded.count, "savedBytes": saved.count, "sessions": snapshot.sessions.count])
        }
    } catch {
        emit(["error": describe(error), "refused": refused] as [String: Any])
        exit(1)
    }
    // Never saved by design: a mirror lives only while connected; the graph run's `shown*` are views, not stored.
    let unsaved: Set<String> = ["RunSession.mirror", "RemoteWorkspaceReference.mirror", "MightyGraphRun.requestBackgroundWaiting",
                                "MightyGraphRun.shownAwaiting", "MightyGraphRun.shownBackgroundWaiting", "MightyGraphRun.shownFanOut",
                                "MightyGraphRun.shownFlowOrder"]
    let unset = coverage.unset.filter { !unsaved.contains($0) }
    emit(["cases": report, "refused": refused, "unset": unset] as [String: Any])
    // A field Swift saves that no case sets (a new one) or a value of this tool's Swift refused: the corpus no longer covers the state.
    if !unset.isEmpty || !refused.isEmpty { exit(3) }
}

// MARK: - The state check

private func readBack(_ data: Data) throws -> Any {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    return try JSONSerialization.jsonObject(with: encoder.encode(StateRepository.decodeSnapshot(data, restoring: true)))
}

private func leaves(_ value: Any, _ path: String, into result: inout [String: Any]) {
    if let object = value as? [String: Any] {
        if object.isEmpty { result[path] = "{}" }
        for (key, item) in object { leaves(item, path + "." + key, into: &result) }
    } else if let array = value as? [Any] {
        if array.isEmpty { result[path] = "[]" }
        for (index, item) in array.enumerated() { leaves(item, "\(path)[\(index)]", into: &result) }
    } else {
        result[path] = value
    }
}

private func withoutControls(_ text: String) -> String {
    String(String.UnicodeScalarView(text.unicodeScalars.filter { $0.value >= 0x20 || [0x09, 0x0A, 0x0D].contains($0.value) }))
}

private func short(_ value: Any?) -> String {
    guard let value else { return "(missing)" }
    let text = "\(value)"
    return text.count > 160 ? String(text.prefix(160)) + "…" : text
}

/// Every `<case>.rust.json` against `<case>.json`, both read back by StateRepository (`decodeSnapshot(restoring: true)`, then
/// JSONEncoder): each value Swift reads from Swift's file must read the same from Rust's.
func stateCheck(_ folder: URL) {
    var cases: [[String: Any]] = [], failed = false
    let names = ((try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? []).filter { $0.hasSuffix(".rust.json") }.sorted()
    if names.isEmpty { emit(["error": "no <case>.rust.json in \(folder.path)"]); exit(1) }
    for name in names {
        let original = folder.appendingPathComponent(String(name.dropLast(".rust.json".count)) + ".json")
        do {
            let swift = try readBack(Data(contentsOf: original)), rust = try readBack(Data(contentsOf: folder.appendingPathComponent(name)))
            var a: [String: Any] = [:], b: [String: Any] = [:]
            leaves(swift, "", into: &a); leaves(rust, "", into: &b)
            var lost: [String] = [], changed: [String] = [], cleaned: [String] = []
            for (path, value) in a.sorted(by: { $0.key < $1.key }) {
                guard let other = b[path] else { lost.append(path); continue }
                if (value as AnyObject).isEqual(other) { continue }
                // The one difference allowed, and reported: C#'s normalize (`Wire.Clean`), which Rust keeps on both OSes, takes the C0
                // control characters other than tab, line feed and return out of a transcript's text and a workspace's name.
                if let text = value as? String, let rust = other as? String, withoutControls(text) == rust {
                    cleaned.append(path)
                } else {
                    changed.append("\(path): swift \(short(value)) rust \(short(other))")
                }
            }
            let added = b.keys.filter { a[$0] == nil }.sorted()
            if !lost.isEmpty || !changed.isEmpty { failed = true }
            cases.append(["case": name, "values": a.count, "lost": lost, "changed": changed, "cleanedOfControls": cleaned,
                          "addedCount": added.count, "added": Array(added.prefix(40))])
        } catch {
            failed = true
            cases.append(["case": name, "error": describe(error)])
        }
    }
    emit(["verdict": failed ? "lost" : "ok", "cases": cases] as [String: Any])
    if failed { exit(1) }
}

// MARK: - The pairing files

private func sha256(_ text: String) -> String { SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined() }

private func deviceObjects(_ devices: [MobileDevice]) -> [[String: Any]] {
    (try? JSONSerialization.jsonObject(with: JSONEncoder().encode(devices))) as? [[String: Any]] ?? []
}

private func text(_ url: URL) -> String { (try? String(contentsOf: url, encoding: .utf8)) ?? "" }

private let pairingFiles = ["host-id", "mobile-remote.key", "relay-host-token.json", "relay-keypair.json", "devices.json"]

/// Swift's own code makes the files in `folder`, and what they hold goes to `expected` (never stdout: it holds the keys).
func pairingWrite(_ folder: URL, expected: URL) async {
    do {
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let service = MobileRemoteService(dataDirectory: folder, hostName: "Parity Mac", defaultRelayURL: "", watchesNetwork: false)
        let key = try await service.loadOrCreateKey()
        let status = await service.status()
        let registry = MobileDeviceRegistry(url: folder.appendingPathComponent("devices.json"))
        let phone = "cGhvbmUtb25lLTAwMDAwMDA"
        guard case .issued = registry.issueToken(clientId: phone, name: "민수의 iPhone / \"Pro\"") else { fail("Swift issued no token", code: 1) }
        registry.touchLegacy()
        guard registry.setPush(phone, handle: "__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA", kinds: [.completed, .permission]) == .saved else {
            fail("Swift saved no push handle", code: 1)
        }
        await service.shutdown()
        let value: [String: Any] = [
            "hostId": text(folder.appendingPathComponent("host-id")), "pairingKey": key,
            "hostToken": text(folder.appendingPathComponent("relay-host-token.json")), "serverId": status.serverId,
            "publicKeyB64": status.publicKeyB64 ?? "", "devices": deviceObjects(registry.all()),
        ]
        try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]).write(to: expected)
        emit(["verdict": "ok", "files": pairingFiles.filter { FileManager.default.fileExists(atPath: folder.appendingPathComponent($0).path) }] as [String: Any])
    } catch {
        fail(describe(error), code: 1)
    }
}

/// Rust's files in `folder`, read by Swift's own code: the same key, server id, public key and phones as `expected` says, and no
/// file changed by the reading (Swift found nothing to mend).
func pairingRead(_ folder: URL, expected: URL) async {
    guard let data = FileManager.default.contents(atPath: expected.path), let want = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
        fail("unreadable \(expected.path)", code: 2)
    }
    let before = pairingFiles.map { FileManager.default.contents(atPath: folder.appendingPathComponent($0).path) }
    var problems: [String] = []
    do {
        let service = MobileRemoteService(dataDirectory: folder, hostName: "Parity Mac", defaultRelayURL: "", watchesNetwork: false)
        let key = try await service.loadOrCreateKey()
        let status = await service.status()
        await service.shutdown()
        if key != want["pairingKey"] as? String { problems.append("the pairing key differs") }
        if status.serverId != sha256(want["hostToken"] as? String ?? "") { problems.append("the server id is not SHA-256 of the host token") }
        if status.publicKeyB64 != want["publicKeyB64"] as? String { problems.append("the public key differs") }
        let devices = deviceObjects(MobileDeviceRegistry(url: folder.appendingPathComponent("devices.json")).all())
        if !(devices as NSArray).isEqual(to: want["devices"] as? [Any] ?? []) { problems.append("the phones differ: \(short(devices))") }
        let after = pairingFiles.map { FileManager.default.contents(atPath: folder.appendingPathComponent($0).path) }
        for (index, name) in pairingFiles.enumerated() where before[index] != after[index] { problems.append("\(name) changed when Swift read it") }
    } catch {
        problems.append(describe(error))
    }
    emit(["verdict": problems.isEmpty ? "ok" : "refused", "problems": problems] as [String: Any])
    if !problems.isEmpty { exit(1) }
}

// MARK: - The defaults keys

/// The UserDefaults keys MightyCore names publicly (the app target's own are listed in Rust's table from the Swift source).
func defaultsKeys() {
    emit(["keys": [
        DelegationSwitch.defaultsKey: "bool", TextAppearance.scaleKey: "double", TextAppearance.familyKey: "string",
        TextAppearance.transcriptFamilyKey: "string", TextAppearance.codeFamilyKey: "string", WebOpenChoiceStore.defaultsKey: "dictionary",
        CompletionWebhook.defaultsKey: "string", AppLanguage.systemInterfaceKey: "array",
    ]])
}
