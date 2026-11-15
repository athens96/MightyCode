// The app update part of the parity checks (W9-A, plan §3 Step 0h): what MightyCode.Core's update code decides, for the Rust port
// (mighty-updates' app_update) to be compared with it. Each mode reads its input and writes C#'s answers:
//
//   update ed25519 <corpus> <out>          Ed25519Verify.Verify's verdict for each case (native/contracts/fixtures/update-ed25519.json)
//   update manifest <file> <keyb64> <out>  AppUpdateManifest.Parse's verdict and the refusal's locale key
//   update version <pairs> <out>           AppVersion.Normalized and Compare for each pair
//   update plan-args <json> <out>          AppUpdateInstallPlan.Create(...).HelperArguments, and TryParse of argument lists
//   update mutex-name <paths> <out>        AppUpdateHelperGuard.MutexName for each folder (case, separators, ß, Turkish ı/İ, `..`)
//   update stage <zip> <arch> <out>        AppUpdateService.Stage's verdict on a package (the 0.4.x stager's acceptance)
//   update install-log <result json> <fixed time> <out>  the helper's install.log line (WinUI\Program.cs:253-270)
//   update rewrite <text> <old> <new> <out>  RenameMigration's path rewrite (plain and JSON-escaped, before a separator; W9-F)
//
// W9 Step 0: every mode answers "not written yet" (exit 2); W9-A writes them. W9-F wrote `rewrite`: <text> is a UTF-8 file holding the
// text, <old> and <new> the folder paths as given, <out> the rewritten text (UTF-8, no mark; mighty-core tests/rename_migration.rs).
using System.Text;
using MightyCode.Core;

namespace MightyCode.Parity;

internal static class UpdateParity
{
    public static int Run(string[] args)
    {
        var mode = args.Length > 0 ? args[0] : "";
        if (mode == "rewrite") return Rewrite(args[1..]);
        Console.Error.WriteLine($"update {mode}: not written yet (W9-A)");
        return 2;
    }

    private static int Rewrite(string[] args)
    {
        if (args.Length != 4) { Console.Error.WriteLine("update rewrite <text> <old> <new> <out>"); return 2; }
        var utf8 = new UTF8Encoding(false);
        File.WriteAllText(args[3], RenameMigration.Rewritten(File.ReadAllText(args[0], utf8), args[1], args[2]), utf8);
        return 0;
    }
}
