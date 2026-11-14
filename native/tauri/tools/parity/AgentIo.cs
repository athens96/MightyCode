// The Agent IO part of the parity checks (W7-C; `agent-io <in> <out>`): AgentIOBinding's argv vectors (Claude's --mcp-config and Codex's
// -c lists per mode, a non-ASCII exe path escaped as Wire.Json escapes it), answered by MightyCode.Core. The stdio servers' transcript is
// tools/parity/mcp.ps1's. Not written yet: W7-C fills it.
namespace MightyCode.Parity;

internal static class AgentIoParity
{
    public static int Run(string input, string output)
    {
        Console.Error.WriteLine($"agent-io parity is not written yet (W7-C): {input} -> {output}");
        return 2;
    }
}
