// The delegation part of the parity checks (W7-A; crates/mighty-delegation/tests/parity.rs): `delegation <in> <out>` reads one vector
// a line, "<kind>\t<arguments…>" with every byte string in hex, runs the MightyCode.Core function the kind names and writes one answer a
// line. The Rust side runs the same vectors through mighty-delegation and compares the lines.
//
//   roundtrip\t<file bytes>                   DelegationFileStore.Load of a delegation-state.json, then Save -> the bytes written (hex),
//                                             or "unreadable"
//   key\t<session>\t<run>\t<task>\t<mode>\t<other agent | ->
//                                             DelegationRequestKey.Make
//   after\t<state>\t<event>                   ChildStates.After by name -> the state's name, or "null"
//   prune\t<file bytes>\t<open panes,…>\t<undoable merged commits,…>\t<headroom>
//                                             DelegationFile.Pruned -> the pruned file's bytes (hex), or "null"
//   rows\t<file bytes>                        DelegationSidebar.Rows -> one "<id>|<parent>|<state>|<task as hex>|<branch>|<checkout as hex>|<canUndo>|<held>|<human>|<agent>" per row, joined by ";"
//   copy\t<bytes>\t<total | ->                DelegationCopy.Of(c, report, 1, bytes, total) -> "<truncated> <originalBytes> <text as hex>"
using System.Text;
using MightyCode.Core;

namespace MightyCode.Parity;

internal static class DelegationParity
{
    private static readonly UTF8Encoding Utf8 = new(false);

    public static int Run(string input, string output)
    {
        var work = Path.Combine(Path.GetTempPath(), "mighty-delegation-parity-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(work);
        try
        {
            var lines = File.ReadAllLines(input).Select(line => Answer(line, work)).ToList();
            File.WriteAllText(output, string.Join("\n", lines) + "\n", Utf8);
            return 0;
        }
        finally { Directory.Delete(work, true); }
    }

    private static string Hex(byte[] bytes) => Convert.ToHexStringLower(bytes);
    private static string Hex(string? text) => text is null ? "-" : Hex(Utf8.GetBytes(text));
    private static byte[] Bytes(string hex) => Convert.FromHexString(hex);
    private static string Text(string hex) => Utf8.GetString(Bytes(hex));

    private static string Answer(string line, string work)
    {
        var part = line.Split('\t');
        switch (part[0])
        {
            case "roundtrip":
            {
                var folder = Path.Combine(work, Guid.NewGuid().ToString("N")); Directory.CreateDirectory(folder);
                var store = new DelegationFileStore(folder);
                File.WriteAllBytes(store.FilePath, Bytes(part[1]));
                try { var file = store.Load(); store.Save(file); return Hex(File.ReadAllBytes(store.FilePath)); }
                catch (DelegationFileException) { return "unreadable"; }
            }
            case "key":
                return DelegationRequestKey.Make(Text(part[1]), Text(part[2]), Text(part[3]), Text(part[4]), part[5] == "-" ? null : Text(part[5]));
            case "after":
            {
                var state = DelegationNames.Parse<ChildState>(part[1])!.Value; var change = Enum.Parse<ChildStateEvent>(part[2]);
                return state.After(change) is { } next ? DelegationNames.Of(next) : "null";
            }
            case "prune":
            {
                var file = Load(Bytes(part[1]), work);
                var open = part[2].Length == 0 ? new HashSet<string>() : part[2].Split(',').ToHashSet();
                var undoable = part[3].Length == 0 ? new HashSet<string>() : part[3].Split(',').ToHashSet();
                var pruned = file.Pruned(new DelegationPruneContext(open, merge => undoable.Contains(merge.MergedCommit)), long.Parse(part[4], System.Globalization.CultureInfo.InvariantCulture));
                return pruned is null ? "null" : Hex(DelegationFileStore.Encode(pruned));
            }
            case "rows":
                return string.Join(";", DelegationSidebar.Rows(Load(Bytes(part[1]), work)).Select(row =>
                    $"{row.Id}|{row.ParentSessionId}|{DelegationNames.Of(row.State)}|{Hex(row.Task)}|{row.ParentBranch}|{Hex(row.ParentCheckout)}|{(row.CanUndo ? 1 : 0)}|{row.HeldNotices}|{(row.StartedByHuman ? 1 : 0)}|{row.Agent}"));
            case "copy":
            {
                var copy = DelegationCopy.Of("c", DelegationCopyKind.Report, 1, Bytes(part[1]), part[2] == "-" ? null : long.Parse(part[2], System.Globalization.CultureInfo.InvariantCulture));
                return $"{(copy.Truncated ? 1 : 0)} {copy.OriginalBytes} {Hex(copy.Text)}";
            }
            default: return "unknown " + part[0];
        }
    }

    private static DelegationFile Load(byte[] bytes, string work)
    {
        var folder = Path.Combine(work, Guid.NewGuid().ToString("N")); Directory.CreateDirectory(folder);
        var store = new DelegationFileStore(folder);
        File.WriteAllBytes(store.FilePath, bytes);
        return store.Load();
    }
}
