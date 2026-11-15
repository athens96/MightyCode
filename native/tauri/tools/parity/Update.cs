// The app update part of the parity checks (W9-A, plan §3 Step 0h): what MightyCode.Core's update code decides, for the Rust port
// (mighty-updates' app_update) to be compared with it. Each mode reads its input file and writes C#'s answers beside the inputs, as the
// fixture native/contracts/fixtures/update-* that mighty-updates' tests/app_update_parity.rs reads (run with the fixture as both <in>
// and <out> to write it again; the answers it held are replaced):
//
//   update ed25519 <corpus> <out>      Ed25519Verify.Verify's verdict for each case ("csharp": true/false; update-ed25519.json)
//   update manifest <cases> <out>      AppUpdateManifest.Parse's verdict on each envelope file named in <cases> (update-manifests/):
//                                      the manifest read, the refusal's locale key with its Korean sentence, or .NET's exception
//   update version <in> <out>          AppVersion.Normalized, AppVersion.Compare's sign and AppUpdateManifest.Allowed (update-versions.json)
//   update plan-args <in> <out>        AppUpdateInstallPlan.Create(...).HelperArguments, and TryParse of argument lists (update-plan.json)
//   update mutex-name <in> <out>       AppUpdateHelperGuard.MutexName for each folder (case, separators, ß, Turkish ı/İ, `..`; update-mutex.json)
//   update stage <cases> <out>         AppUpdateService.Stage's verdict on each package named in <cases> (the 0.4.x stager's acceptance;
//                                      update-stage/), each copied into a fresh temporary folder first
//   update install-log <in> <out>      the helper's install.log line (WinUI\Program.cs:253-270, its interpolation restated here) at a
//                                      fixed time given in .NET ticks (update-install-log.json)
//   update rewrite <text> <old> <new> <out>  RenameMigration's path rewrite (W9-F's; not written yet)
//
// The refusals are worded in Korean (Locale.LanguagePreference = "ko"), as Core.Tests reads them.
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using MightyCode.Core;

namespace MightyCode.Parity;

internal static class UpdateParity
{
    private static readonly JsonSerializerOptions Indented = new() { WriteIndented = true, Encoder = System.Text.Encodings.Web.JavaScriptEncoder.UnsafeRelaxedJsonEscaping };

    public static int Run(string[] args)
    {
        Locale.LanguagePreference = "ko"; Locale.ResetCache();
        var mode = args.Length > 0 ? args[0] : "";
        switch (mode)
        {
            case "ed25519" when args.Length == 3: return Write(args[2], Ed25519(Read(args[1])));
            case "manifest" when args.Length == 3: return Write(args[2], Manifest(args[1]));
            case "version" when args.Length == 3: return Write(args[2], Version(Read(args[1])));
            case "plan-args" when args.Length == 3: return Write(args[2], PlanArgs(Read(args[1])));
            case "mutex-name" when args.Length == 3: return Write(args[2], MutexName(Read(args[1])));
            case "stage" when args.Length == 3: return Write(args[2], Stage(args[1]));
            case "install-log" when args.Length == 3: return Write(args[2], InstallLog(Read(args[1])));
            case "rewrite":
                Console.Error.WriteLine("update rewrite: not written yet (W9-F)");
                return 2;
            default:
                Console.Error.WriteLine("usage: update ed25519|manifest|version|plan-args|mutex-name|stage|install-log <in> <out>");
                return 2;
        }
    }

    private static JsonNode Read(string path) => JsonNode.Parse(File.ReadAllText(path))!;

    private static int Write(string path, JsonNode node)
    {
        File.WriteAllText(path, node.ToJsonString(Indented).Replace("\r\n", "\n") + "\n", new UTF8Encoding(false));
        return 0;
    }

    /// <summary>The locale keys an update refusal may be; a sentence is matched against each one's Korean words, `{name}` standing for anything.</summary>
    private static readonly string[] RefusalKeys =
    [
        "appUpdate.error.manifestFileTooLarge", "appUpdate.error.manifestFileNotJSON", "appUpdate.error.signatureFormat",
        "appUpdate.error.signatureEncoding", "appUpdate.error.signatureMismatch", "appUpdate.error.unsigned",
        "appUpdate.error.manifestNotJSON", "appUpdate.error.noVersion", "appUpdate.error.executableCount",
        "appUpdate.error.executableLocation", "appUpdate.error.entryAbsolute", "appUpdate.error.entryParent",
        "appUpdate.error.entryLink", "appUpdate.error.entryOutside", "appUpdate.error.executableDamaged", "appUpdate.error.architecture",
    ];

    private static string? KeyOf(string message)
    {
        foreach (var key in RefusalKeys)
        {
            var pattern = "^" + System.Text.RegularExpressions.Regex.Replace(System.Text.RegularExpressions.Regex.Escape(Locale.Get(key)), @"\\\{[A-Za-z_]+}", ".*") + "$";
            if (System.Text.RegularExpressions.Regex.IsMatch(message, pattern, System.Text.RegularExpressions.RegexOptions.Singleline)) return key;
        }
        return null;
    }

    /// <summary>A refusal as the fixture keeps it: the locale key and its sentence, or the exception's type and text.</summary>
    private static JsonObject Refusal(Exception error) =>
        error is InvalidOperationException && KeyOf(error.Message) is { } key
            ? new JsonObject { ["verdict"] = "key", ["key"] = key, ["message"] = error.Message }
            : new JsonObject { ["verdict"] = "exception", ["type"] = error.GetType().Name, ["message"] = error.Message };

    private static JsonNode Ed25519(JsonNode corpus)
    {
        foreach (var item in corpus["cases"]!.AsArray())
        {
            var node = item!.AsObject();
            node["csharp"] = Ed25519Verify.Verify(
                Convert.FromHexString((string)node["message"]!), Convert.FromHexString((string)node["signature"]!), Convert.FromHexString((string)node["key"]!));
        }
        return corpus;
    }

    private static JsonNode Manifest(string casesPath)
    {
        var cases = Read(casesPath);
        var folder = Path.GetDirectoryName(Path.GetFullPath(casesPath))!;
        var keys = new Dictionary<string, byte[]> { ["fixture"] = Convert.FromBase64String((string)cases["fixture"]!), ["other"] = Convert.FromBase64String((string)cases["other"]!) };
        var answers = new JsonArray();
        foreach (var item in cases["cases"]!.AsArray())
        {
            var name = (string)item!["name"]!;
            var data = File.ReadAllBytes(Path.Combine(folder, (string)item["file"]!));
            JsonObject answer;
            try
            {
                var manifest = AppUpdateManifest.Parse(data, keys[(string?)item["with"] ?? "fixture"]);
                var windows = new JsonObject();
                foreach (var (architecture, asset) in manifest.Windows.OrderBy(pair => pair.Key, StringComparer.Ordinal))
                    windows[architecture] = new JsonObject { ["url"] = asset.Url, ["sha256"] = asset.Sha256, ["size"] = asset.Size };
                answer = new JsonObject
                {
                    ["verdict"] = "ok",
                    ["version"] = manifest.Version,
                    ["build"] = manifest.Build,
                    ["notes"] = manifest.Notes,
                    ["publishedAt"] = manifest.PublishedAt,
                    ["windows"] = windows,
                };
            }
            catch (Exception error) { answer = Refusal(error); }
            answer.Insert(0, "name", name);
            answers.Add(answer);
        }
        cases["csharp"] = answers;
        return cases;
    }

    private static JsonNode Version(JsonNode input)
    {
        foreach (var item in input["normalize"]!.AsArray())
            item!["csharp"] = AppVersion.Normalized((string)item["text"]!);
        foreach (var item in input["compare"]!.AsArray())
            item!["csharp"] = Math.Sign(AppVersion.Compare((string)item["left"]!, (string)item["right"]!));
        foreach (var item in input["allowed"]!.AsArray())
            item!["csharp"] = AppUpdateManifest.Allowed((string)item["url"]!);
        return input;
    }

    private static JsonObject PlanNode(AppUpdateInstallPlan plan) => new()
    {
        ["staged"] = plan.StagedDirectory,
        ["install"] = plan.InstallDirectory,
        ["backup"] = plan.BackupDirectory,
        ["package"] = plan.PackagePath,
        ["sha256"] = plan.Sha256,
        ["size"] = plan.Size,
        ["pid"] = plan.AppProcessId,
        ["newExecutable"] = plan.NewExecutable,
        ["previousExecutable"] = plan.PreviousExecutable,
        ["relaunch"] = plan.Relaunch,
        ["newDirectory"] = plan.NewDirectory,
        ["stagedHelper"] = plan.StagedHelperExecutable,
    };

    private static JsonNode PlanArgs(JsonNode input)
    {
        foreach (var item in input["create"]!.AsArray())
        {
            var plan = AppUpdateInstallPlan.Create(
                (string)item!["staged"]!, (string)item["install"]!, (string)item["package"]!, (string)item["sha256"]!,
                (long)item["size"]!, (int)item["pid"]!, (string)item["backup"]!) with { Relaunch = (bool)item["relaunch"]! };
            item["csharp"] = new JsonArray([.. plan.HelperArguments.Select(argument => (JsonNode)JsonValue.Create(argument)!)]);
        }
        foreach (var item in input["parse"]!.AsArray())
        {
            var arguments = item!["arguments"]!.AsArray().Select(argument => (string)argument!).ToList();
            try { item["csharp"] = AppUpdateInstallPlan.TryParse(arguments) is { } plan ? PlanNode(plan) : null; }
            catch (Exception error) { item["csharp"] = new JsonObject { ["exception"] = error.GetType().Name }; }
        }
        return input;
    }

    private static JsonNode MutexName(JsonNode input)
    {
        foreach (var item in input["paths"]!.AsArray())
            item!["csharp"] = AppUpdateHelperGuard.MutexName((string)item["path"]!);
        return input;
    }

    private static JsonNode Stage(string casesPath)
    {
        var cases = Read(casesPath);
        var folder = Path.GetDirectoryName(Path.GetFullPath(casesPath))!;
        foreach (var item in cases["cases"]!.AsArray())
        {
            var work = Path.Combine(Path.GetTempPath(), "mighty-parity-stage-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(work);
            try
            {
                var package = Path.Combine(work, "package.zip");
                File.Copy(Path.Combine(folder, (string)item!["zip"]!), package);
                JsonObject answer;
                try
                {
                    var app = AppUpdateService.Stage(package, (string)item["arch"]!);
                    var root = Path.Combine(work, "staged");
                    var files = Directory.GetFiles(root, "*", SearchOption.AllDirectories)
                        .Select(file => Path.GetRelativePath(root, file).Replace('\\', '/')).OrderBy(file => file, StringComparer.Ordinal);
                    answer = new JsonObject
                    {
                        ["verdict"] = "ok",
                        ["folder"] = Path.GetRelativePath(root, app).Replace('\\', '/'),
                        ["files"] = new JsonArray([.. files.Select(file => (JsonNode)JsonValue.Create(file)!)]),
                    };
                }
                catch (Exception error) { answer = Refusal(error); }
                item["csharp"] = answer;
            }
            finally { Directory.Delete(work, true); }
        }
        return cases;
    }

    private static JsonNode InstallLog(JsonNode input)
    {
        foreach (var item in input["cases"]!.AsArray())
        {
            var time = new DateTimeOffset((long)item!["ticks"]!, TimeSpan.Zero);
            var (replaced, rolledBack, relaunch) = ((bool)item["replaced"]!, (bool)item["rolledBack"]!, (bool)item["relaunch"]!);
            var (skipped, started, backupRemoved) = ((bool)item["skipped"]!, (bool)item["started"]!, (bool)item["backupRemoved"]!);
            var error = (string?)item["error"];
            item["csharp"] = $"{time:O} replaced={replaced} rolledBack={rolledBack} " +
                $"relaunch={relaunch} skipped={skipped} started={started} backupRemoved={backupRemoved} {error}\n";
        }
        return input;
    }
}
