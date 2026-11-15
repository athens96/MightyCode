// The styles part of the parity checks (W9-D, plan §3 Step 0h, R9): what MightyCode.Core's style engine projects, for the Rust port
// (mighty-styles' `examples/parity.rs`, the same arguments) to be compared with it byte for byte.
//
//   styles project <manifest> <state> <out>   StylePanelProjection's panel bytes (Serialise) for a manifest in a state
//
// <state> is a JSON object, every field optional: source ("user"), prompts ([]), group (null), running (false), ready (true),
// missing ([]), hint (null), stage (null: a plan stage read with the golden's fixed run state), checklist (null: every file source
// parses this text), events ([[type, value], ...]), language ("ko"), vocabulary ({"acme.thing": [...]}: decoded under it).
using System.Text;
using System.Text.Json;
using MightyCode.Core;

namespace MightyCode.Parity;

internal static class StylesParity
{
    public static int Run(string[] args)
    {
        var mode = args.Length > 0 ? args[0] : "";
        if (mode != "project" || args.Length != 4) { Console.Error.WriteLine("usage: styles project <manifest> <state> <out>"); return 2; }
        var bytes = File.ReadAllBytes(args[1]);
        using var state = JsonDocument.Parse(File.ReadAllBytes(args[2]));
        var s = state.RootElement;
        string? Text(string name) => s.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
        bool Flag(string name, bool fallback) => s.TryGetProperty(name, out var v) && v.ValueKind is JsonValueKind.True or JsonValueKind.False ? v.GetBoolean() : fallback;
        string[] List(string name) => s.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Array ? v.EnumerateArray().Select(e => e.GetString() ?? "").ToArray() : [];
        var saved = Locale.LanguagePreference; Locale.LanguagePreference = Text("language") ?? "ko"; Locale.ResetCache();
        try
        {
            Dictionary<string, string[]>? vocabulary = null;
            if (s.TryGetProperty("vocabulary", out var v) && v.ValueKind == JsonValueKind.Object)
                vocabulary = v.EnumerateObject().ToDictionary(p => p.Name, p => p.Value.EnumerateArray().Select(e => e.GetString()!).ToArray());
            var source = Text("source") ?? "user";
            var style = new RegisteredStyle(StyleManifestDecoder.Decode(bytes, source, vocabulary), source, args[1], null, Convert.ToHexStringLower(System.Security.Cryptography.SHA256.HashData(bytes)), source == "bundled" ? "preApproved" : "approved", bytes);
            StyleStateReading? reading = null;
            if (style.Manifest.Root.TryGetProperty("stateSources", out var sources) && (Text("checklist") is not null || Text("stage") is not null || s.TryGetProperty("events", out _)))
            {
                var checklist = Text("checklist") ?? "";
                var files = StyleManifest.Items(sources, "files").Select(f => StyleStateEngine.Parse(checklist, StyleManifest.Text(f, "parser")!, StyleManifest.Text(f, "widget")!)).ToArray();
                var events = s.TryGetProperty("events", out var e) ? e.EnumerateArray().Select(p => (p[0].GetString()!, p[1].GetString()!)).ToArray() : [];
                var items = Enumerable.Range(1, 7).Select(i => new TodoItem("step " + i, i <= 3 ? "completed" : i == 4 ? "in_progress" : "pending", "doing step " + i)).ToList();
                var work = new BackgroundWork([new BackgroundTask("bg-1", "agent", "review the diff", "2026-10-06T01:00:00.000Z"), new BackgroundTask("bg-2", "shell", "npm test", "2026-10-06T01:00:05.000Z", "completed", EndedAt: "2026-10-06T01:01:10.000Z")], TurnEnded: true);
                reading = Text("stage") is { } stage && style.Evaluator.ReadsPlanState ? StyleStateEngine.Reading(style.Manifest, files, events, stage, new TodoProgress(items), work) : StyleStateEngine.Reading(style.Manifest, files, events);
            }
            var prerequisites = new StylePrerequisiteResult(Flag("ready", true), List("missing"), Text("hint"), null);
            var panel = StylePanelProjection.Make(style, List("prompts"), Text("group"), new Dictionary<string, string>(), prerequisites, Flag("running", false), null, reading);
            File.WriteAllBytes(args[3], StylePanelProjection.Serialise(panel));
            return 0;
        }
        catch (StyleManifestException e) { File.WriteAllText(args[3], e.Code + "\n", new UTF8Encoding(false)); return 0; }
        finally { Locale.LanguagePreference = saved; Locale.ResetCache(); }
    }
}
