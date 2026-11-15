// The styles part of the parity checks (W9-D, plan §3 Step 0h, R9): what MightyCode.Core's style engine projects, for the Rust port
// (mighty-styles) to be compared with it byte for byte.
//
//   styles project <manifest> <state> <out>   StylePanelProjection's panel JSON for a manifest in a state (every golden case of
//                                              styles/golden; decisive for the panel bytes)
//
// W9 Step 0: the mode answers "not written yet" (exit 2); W9-D writes it.
namespace MightyCode.Parity;

internal static class StylesParity
{
    public static int Run(string[] args)
    {
        var mode = args.Length > 0 ? args[0] : "";
        Console.Error.WriteLine($"styles {mode}: not written yet (W9-D)");
        return 2;
    }
}
