// The remote workspaces part of the parity checks (W8-D, D18; `remote <in> <out>`): RemoteHttpListener's head parser over a corpus of
// malformed heads, answered by MightyCode.Core with the status and whether the body is read; Rust's strict server answers the same.
// Not written yet: W8-D fills it.
namespace MightyCode.Parity;

internal static class RemoteParity
{
    public static int Run(string input, string output)
    {
        Console.Error.WriteLine($"remote parity is not written yet (W8-D): {input} -> {output}");
        return 2;
    }
}
