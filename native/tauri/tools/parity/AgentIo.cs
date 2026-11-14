// The Agent IO part of the parity checks (W7-C; `cargo run -p mighty-delegation --example agent_io_parity -- <in> <out>` is the Rust
// side, tools/parity/mcp.ps1 runs both): <in> is a folder of
//   bindings.jsonl    one binding a line {provider, mode, attached, executable} -> AgentIOBinding.AppendArguments' argv as a JSON array
//   terminal.jsonl    one JSON-RPC line a line -> AgentIOMcp.Handle's answer as MainAsync builds the server with no pipe in the
//                     environment ("<none>" where it gives none or throws, which C#'s loop answers with nothing)
//   delegation.jsonl  the same through DelegationMcp, started without a pane connection
// A line "@oversize" stands for a ping 1 MiB long. Each output file in <out> has one answer a line.
using System.Text;
using System.Text.Json;
using MightyCode.Core;

namespace MightyCode.Parity;

internal static class AgentIoParity
{
    private static readonly UTF8Encoding Utf8 = new(false);

    public static int Run(string input, string output)
    {
        Directory.CreateDirectory(output);
        var bindings = new List<string>();
        foreach (var line in File.ReadAllLines(Path.Combine(input, "bindings.jsonl"), Utf8).Where(line => line.Length > 0))
        {
            using var vector = JsonDocument.Parse(line);
            var root = vector.RootElement;
            var registry = new AgentIOBindings();
            var binding = registry.Bind("pane", new Workspace { Id = "ws", Path = Path.GetTempPath() }, root.GetProperty("provider").GetString()!, "pipe", root.GetProperty("executable").GetString()!);
            binding.ConnectDelegation(new DelegationIOHandler(() => true), root.GetProperty("attached").GetBoolean(), root.GetProperty("mode").GetString()!);
            var arguments = new List<string>(); binding.AppendArguments(arguments);
            bindings.Add(JsonSerializer.Serialize(arguments));
        }
        File.WriteAllText(Path.Combine(output, "bindings.txt"), string.Join("\n", bindings) + "\n", Utf8);
        var terminal = new AgentIOMcp((tool, args, cancellation) => AgentIOPipe.Call("", "", tool, args, cancellation));
        Transcript(Path.Combine(input, "terminal.jsonl"), Path.Combine(output, "terminal.txt"), line => terminal.Handle(line));
        var delegation = new DelegationMcp(null);
        Transcript(Path.Combine(input, "delegation.jsonl"), Path.Combine(output, "delegation.txt"), line => delegation.Handle(line));
        return 0;
    }

    /// The ping a line "@oversize" stands for: one byte over 1 MiB.
    internal static string Oversize() => "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"x\":\"" + new string('x', AgentIOPipe.MaximumBytes) + "\"}";

    private static void Transcript(string input, string output, Func<string, Task<string?>> handle)
    {
        var answers = new List<string>();
        foreach (var raw in File.ReadAllText(input, Utf8).Split('\n'))
        {
            if (raw.Length == 0) continue;
            var line = raw == "@oversize" ? Oversize() : raw;
            string? answer;
            try { answer = handle(line).GetAwaiter().GetResult(); }
            catch (Exception ex) when (ex is not OutOfMemoryException) { answer = null; }
            answers.Add(answer ?? "<none>");
        }
        File.WriteAllText(output, string.Join("\n", answers) + "\n", Utf8);
    }
}
