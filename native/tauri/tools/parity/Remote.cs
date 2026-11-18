// The remote workspaces part of the parity checks (W8-D, D18; `remote <in> <out>`): RemoteHttpListener's head parser over a corpus of
// raw requests (native/contracts/fixtures/remote-heads/heads.jsonl: one {"name", "hex"} a line), each sent on its own loopback connection
// to a listener whose handler answers 200 with "len:<body bytes>". Writes one line a case: its name, the answer's status line (or
// "closed" with nothing answered, "pending" when nothing came in 3 s) and its body in lower-case hex, tab-separated. The answers are kept
// as native/contracts/fixtures/remote-heads/heads-csharp.txt, which mighty-remote's tests/remote_parity.rs checks Rust's strict server
// against (run.ps1 makes them again and compares). A case with "limit": "host" goes to a second listener whose body limit is the sharing
// host's own (RemoteHostService.BodyLimit with the key below, loopback allowed): none without the key, the larger one for a run.
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json.Nodes;
using MightyCode.Core;

namespace MightyCode.Parity;

internal static class RemoteParity
{
    /// <summary>The key the host-limit cases present (tests/remote_parity.rs's KEY).</summary>
    private const string Key = "parity-remote-key-0123456789abcdefghijklmnop";

    public static int Run(string input, string output)
    {
        var lines = File.ReadAllLines(input).Where(l => l.Length > 0).ToList();
        var answers = new List<string>();
        static Task<RemoteHttpResponse> Handler(RemoteHttpRequest request) =>
            Task.FromResult(new RemoteHttpResponse(200, Encoding.UTF8.GetBytes("len:" + request.Body.Length.ToString(System.Globalization.CultureInfo.InvariantCulture))));
        var listener = new RemoteHttpListener(IPAddress.Loopback, 0, Handler);
        var hosted = new RemoteHttpListener(IPAddress.Loopback, 0, Handler, head => RemoteHostService.BodyLimit(head, Key, true));
        var port = listener.Start();
        var hostPort = hosted.Start();
        try
        {
            foreach (var line in lines)
            {
                var node = JsonNode.Parse(line)!;
                var name = (string)node["name"]!;
                var raw = Convert.FromHexString((string)node["hex"]!);
                var target = (string?)node["limit"] == "host" ? hostPort : port;
                answers.Add(name + "\t" + Exchange(target, raw).GetAwaiter().GetResult());
            }
        }
        finally
        {
            listener.DisposeAsync().AsTask().GetAwaiter().GetResult();
            hosted.DisposeAsync().AsTask().GetAwaiter().GetResult();
        }
        File.WriteAllText(output, string.Join("\n", answers) + "\n", new UTF8Encoding(false));
        return 0;
    }

    /// <summary>One exchange: the bytes sent, then everything read until the server closes (or 3 s pass with nothing more).</summary>
    private static async Task<string> Exchange(int port, byte[] raw)
    {
        using var client = new TcpClient();
        await client.ConnectAsync(IPAddress.Loopback, port);
        var stream = client.GetStream();
        try { await stream.WriteAsync(raw); } catch (IOException) { }
        var received = new MemoryStream();
        var buffer = new byte[65536];
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(3));
        try
        {
            while (true)
            {
                var read = await stream.ReadAsync(buffer, timeout.Token);
                if (read == 0) break;
                received.Write(buffer, 0, read);
            }
        }
        catch (OperationCanceledException) { if (received.Length == 0) return "pending\t"; }
        catch (IOException) { }
        var text = received.ToArray();
        if (text.Length == 0) return "closed\t";
        var head = Array.IndexOf(text, (byte)'\r') is var end and >= 0 ? Encoding.ASCII.GetString(text, 0, end) : Encoding.ASCII.GetString(text);
        var split = FindBody(text);
        var body = split < 0 ? "" : Convert.ToHexString(text, split, text.Length - split).ToLowerInvariant();
        return head + "\t" + body;
    }

    private static int FindBody(byte[] text)
    {
        for (var i = 0; i + 3 < text.Length; i++)
            if (text[i] == '\r' && text[i + 1] == '\n' && text[i + 2] == '\r' && text[i + 3] == '\n') return i + 4;
        return -1;
    }
}
