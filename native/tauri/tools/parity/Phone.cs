// The phone part of the parity checks (W8-A and W8-B): MightyCode.Core answering
//   push <in> <out>                    W8-A (crates/mighty-remote/tests/parity.rs): one vector a line, every string in hex —
//                                        encode\t<id>\t<kind>\t<hostId>\t<hostName>\t<workspace>\t<title>\t<summary>\t<sessionId>\t<permission>\t<at>
//                                          (<permission> "-" or "<requestId>,<runId>,<buttons>") -> MobilePushMessage.EncodedForPush (hex), or "null"
//                                        summary\t<text>  -> the summary MobilePushPlanner gives a new request with that text (hex)
//                                      Rust compares the lines.
//   pairing-url <in> <out>             W8-A: one vector a line, "<mobile-identity.json bytes>\t<relay>\t<name>\t<direct, comma-joined | ->"
//                                      (hex) -> MobileIdentity(folder holding that file).PairingUrl(relay, name, direct), or "invalid" when
//                                      the constructor refuses the file.
//   routes <profile> <requests> <out>  W8-B: MobileDesktopRouter over a DesktopService loaded from <profile> with no runs, answering each
//                                      GET of <requests>.jsonl (/m1/info, /m1/state, /m1/sessions/{id}, entries, commands, files); the
//                                      Rust headless desk answers the same, compared as JSON values, then as bytes.
using System.Text;
using System.Text.Json;
using MightyCode.Core;

namespace MightyCode.Parity;

internal static class PhoneParity
{
    private static readonly UTF8Encoding Utf8 = new(false);
    private static string Hex(byte[] bytes) => Convert.ToHexStringLower(bytes);
    private static string Text(string hex) => Utf8.GetString(Convert.FromHexString(hex));

    public static int Push(string input, string output)
    {
        var lines = File.ReadAllLines(input).Where(line => line.Length > 0).Select(line =>
        {
            var part = line.Split('\t');
            switch (part[0])
            {
                case "encode":
                {
                    MobilePushPermission? permission = null;
                    if (part[9] != "-") { var p = part[9].Split(','); permission = new(Text(p[0]), Text(p[1]), Text(p[2])); }
                    var message = new MobilePushMessage(Text(part[1]), Text(part[2]), Text(part[3]), Text(part[4]), Text(part[5]), Text(part[6]), Text(part[7]), Text(part[8]), permission, Text(part[10]));
                    return message.EncodedForPush() is { } bytes ? Hex(bytes) : "null";
                }
                case "summary":
                {
                    var planner = new MobilePushPlanner();
                    var pane = new MobilePushPane("p", "w", "t", "running", null, [], false);
                    planner.Observe([pane], "h", "n", "a");
                    var sent = planner.Observe([pane with { Requests = [new("r", "u", MobilePushKinds.Permission, Text(part[1]), MobilePushButtonKinds.Claude)] }], "h", "n", "a");
                    return Hex(Utf8.GetBytes(sent.Single().Summary));
                }
                default: throw new InvalidOperationException("unknown push vector " + part[0]);
            }
        }).ToList();
        File.WriteAllText(output, string.Join("\n", lines) + "\n", Utf8);
        return 0;
    }

    public static int PairingUrl(string input, string output)
    {
        var work = Path.Combine(Path.GetTempPath(), "mighty-pairing-parity-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(work);
        try
        {
            var lines = File.ReadAllLines(input).Where(line => line.Length > 0).Select(line =>
            {
                var part = line.Split('\t');
                var folder = Path.Combine(work, Guid.NewGuid().ToString("N")); Directory.CreateDirectory(folder);
                File.WriteAllBytes(Path.Combine(folder, "mobile-identity.json"), Convert.FromHexString(part[0]));
                MobileIdentity identity;
                try { identity = new MobileIdentity(folder); }
                catch (Exception ex) when (ex is IOException or JsonException or FormatException or NullReferenceException or ArgumentException) { return "invalid"; }
                return identity.PairingUrl(Text(part[1]), Text(part[2]), part[3] == "-" ? null : Text(part[3]).Split(','));
            }).ToList();
            File.WriteAllText(output, string.Join("\n", lines) + "\n", Utf8);
            return 0;
        }
        finally { Directory.Delete(work, true); }
    }

    public static int Routes(string profile, string requests, string output) => RoutesAsync(profile, requests, output).GetAwaiter().GetResult();

    /// <summary>
    /// The router over a copy of the profile folder (its <c>workspace-state.json</c>), no runs, no window: <c>action</c> answers null and
    /// <c>extra</c> is <see cref="Extras"/>, the host is "Parity", id "host-parity", version 0.0.0 — what the Rust check's headless desk answers with.
    /// Each line of <paramref name="requests"/> is a GET path; each line written is <c>status	body</c> (the body as <c>Wire.Json</c> writes it).
    /// </summary>
    /// <summary>
    /// The window's extras as a fixed function of the pane, the Rust check's desk giving the same: two queued rows (one with its 바로 실행,
    /// one unavailable), the settings record, the Mighty payload with the pane's runs (<see cref="MobileMightySupport.Runs"/> over its saved
    /// or transcript runs), a status line, one ask target, and the background work and done count of its record.
    /// </summary>
    private static MobilePaneExtras Extras(RunSession pane) => new(
        [new MobileQueuedItem("q-1", "Next request", "here"), new MobileQueuedItem("q-2", "Later 한글", "unavailable", "turn")],
        Settings: new MobileSettings(true, pane.Model, pane.Settings.PermissionMode, null, "mighty", "cli", "cli",
            new([new("default", "Default")], [new("manual", "Manual")], null, [new("cli", "CLI")], [new("cli", "cli")])),
        Mighty: new { style = "cli", styleId = "cli", runs = MobileMightySupport.Runs(MobileMightySupport.SavedRuns(pane)) },
        StatusLine: new { lines = new[] { new[] { new { text = "main <branch>", fg = "#FFAA00", bold = true } } } },
        AskTargets: [new MobileAskTarget("codex", "Codex")], AskTarget: "codex",
        BackgroundWork: PlanCardSupport.MobileBackgroundWork(pane.BackgroundWork, MightyGraphSupport.NewestRequestRun(pane.GraphRuns)),
        BackgroundDone: PlanCardSupport.MobileBackgroundDone(pane.BackgroundWork));

    private static async Task<int> RoutesAsync(string profile, string requests, string output)
    {
        var work = Path.Combine(Path.GetTempPath(), "mighty-routes-parity-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(work);
        try
        {
            foreach (var file in Directory.EnumerateFiles(profile)) File.Copy(file, Path.Combine(work, Path.GetFileName(file)));
            await using var service = new DesktopService(work, null, work);
            await service.InitializeAsync();
            using var router = new MobileDesktopRouter(service, Path.Combine(work, "mobile"), "Parity", (_, _) => Task.FromResult<object?>(null), Extras)
            { AppVersion = "0.0.0", HostId = "host-parity", ScreenShareAvailable = false };
            var lines = new List<string>();
            foreach (var path in File.ReadAllLines(requests).Where(line => line.Length > 0))
            {
                var reply = await router.RouteAsync("GET", path, null, "phone", CancellationToken.None);
                lines.Add(reply.Status + "\t" + JsonSerializer.Serialize(reply.Body, Wire.Json));
            }
            File.WriteAllText(output, string.Join("\n", lines) + "\n", Utf8);
            return 0;
        }
        finally { try { Directory.Delete(work, true); } catch (IOException) { } }
    }
}
