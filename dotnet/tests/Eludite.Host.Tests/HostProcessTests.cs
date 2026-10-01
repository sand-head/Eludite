using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace Niello.Host.Tests;

/// <summary>
/// The real niello-host process, without a language server: stdout carries nothing but framed JSON-RPC messages
/// (CLAUDE.md invariant 10), and the renamed lifecycle exits with code 0.
/// </summary>
public sealed class HostProcessTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Fact]
    public async Task Stdout_IsProtocolOnly_AndRenamedLifecycleExitsCleanly()
    {
        var psi = new ProcessStartInfo("dotnet")
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        psi.ArgumentList.Add(Path.Combine(AppContext.BaseDirectory, "niello-host.dll"));
        psi.ArgumentList.Add("--stdio");
        psi.ArgumentList.Add("--no-roslyn");
        using var host = Process.Start(psi)!;
        var stdout = new MemoryStream();
        var gate = new object();
        var copy = Task.Run(
            async () =>
            {
                var buffer = new byte[8192];
                int read;
                while ((read = await host.StandardOutput.BaseStream.ReadAsync(buffer, Ct)) > 0)
                {
                    lock (gate)
                    {
                        stdout.Write(buffer, 0, read);
                    }
                }
            },
            Ct);
        var stderr = host.StandardError.ReadToEndAsync(Ct);
        var input = host.StandardInput.BaseStream;
        var dir = Directory.CreateTempSubdirectory("niello-0007-");
        try
        {
            var sln = Path.Combine(dir.FullName, "App.slnx");
            await File.WriteAllTextAsync(sln, "<Solution />", Ct);
            await SendAsync(input, new { jsonrpc = "2.0", id = 1, method = "niello/host/initialize", @params = new { clientName = "t", clientVersion = "0" } });
            await SendAsync(input, new { jsonrpc = "2.0", id = 2, method = "niello/ping" });
            await SendAsync(input, new { jsonrpc = "2.0", id = 3, method = "niello/solution/open", @params = new { path = sln } });
            await SendAsync(input, new { jsonrpc = "2.0", id = 4, method = "initialize", @params = new { } });
            await WaitForResponseAsync(stdout, gate, id: 4);
            await SendAsync(input, new { jsonrpc = "2.0", id = 5, method = "niello/host/shutdown" });
            await WaitForResponseAsync(stdout, gate, id: 5);
            await SendAsync(input, new { jsonrpc = "2.0", method = "niello/host/exit" });
            await host.WaitForExitAsync(Ct).WaitAsync(TimeSpan.FromSeconds(30), Ct);
            await copy;

            Assert.Equal(0, host.ExitCode);
            var messages = ParseFrames(stdout.ToArray());
            var byId = messages.Where(m => m.TryGetProperty("id", out _)).ToDictionary(m => m.GetProperty("id").GetInt32());
            Assert.Equal("niello-host", byId[1].GetProperty("result").GetProperty("hostName").GetString());
            Assert.True(byId[2].GetProperty("result").GetProperty("pong").GetBoolean());
            Assert.Equal(1, byId[3].GetProperty("result").GetProperty("generation").GetInt64());
            Assert.Equal(-32601, byId[4].GetProperty("error").GetProperty("code").GetInt32());
            Assert.Equal(JsonValueKind.Null, byId[5].GetProperty("result").ValueKind);
            var methods = messages.Where(m => m.TryGetProperty("method", out _)).Select(m => m.GetProperty("method").GetString()).ToList();
            Assert.Contains("niello/languageServer/status", methods);
            Assert.Contains("niello/solution/status", methods);
            Assert.Contains("listening on stdio", await stderr, StringComparison.Ordinal);
        }
        finally
        {
            if (!host.HasExited)
            {
                host.Kill(entireProcessTree: true);
            }

            dir.Delete(recursive: true);
        }
    }

    /// <summary>Polls the captured stdout until a response with the given id has arrived, so the test never relies on sleeps.</summary>
    private static async Task WaitForResponseAsync(MemoryStream stdout, object gate, int id)
    {
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(30);
        while (DateTime.UtcNow < deadline)
        {
            byte[] snapshot;
            lock (gate)
            {
                snapshot = stdout.ToArray();
            }

            if (ParseCompleteFrames(snapshot).Any(m => m.TryGetProperty("id", out var i) && i.ValueKind == JsonValueKind.Number && i.GetInt32() == id))
            {
                return;
            }

            await Task.Delay(25, Ct);
        }

        Assert.Fail($"no response with id {id} within 30 s");
    }

    /// <summary>Like <see cref="ParseFrames"/> but stops at a partial trailing frame instead of asserting.</summary>
    private static List<JsonElement> ParseCompleteFrames(byte[] bytes)
    {
        var messages = new List<JsonElement>();
        var at = 0;
        while (at < bytes.Length)
        {
            var headerEnd = bytes.AsSpan(at).IndexOf("\r\n\r\n"u8);
            if (headerEnd <= 0)
            {
                break;
            }

            var length = -1;
            foreach (var line in Encoding.ASCII.GetString(bytes, at, headerEnd).Split("\r\n"))
            {
                var parts = line.Split(':', 2);
                if (parts.Length == 2 && parts[0].Trim().Equals("Content-Length", StringComparison.OrdinalIgnoreCase))
                {
                    length = int.Parse(parts[1].Trim(), System.Globalization.CultureInfo.InvariantCulture);
                }
            }

            var start = at + headerEnd + 4;
            if (length < 0 || start + length > bytes.Length)
            {
                break;
            }

            messages.Add(JsonDocument.Parse(bytes.AsMemory(start, length)).RootElement.Clone());
            at = start + length;
        }

        return messages;
    }

    private static async Task SendAsync(Stream input, object message)
    {
        var body = JsonSerializer.SerializeToUtf8Bytes(message);
        await input.WriteAsync(Encoding.ASCII.GetBytes($"Content-Length: {body.Length}\r\n\r\n"), Ct);
        await input.WriteAsync(body, Ct);
        await input.FlushAsync(Ct);
    }

    /// <summary>Parses every byte as Content-Length frames; fails on anything else.</summary>
    private static List<JsonElement> ParseFrames(byte[] bytes)
    {
        var messages = new List<JsonElement>();
        var at = 0;
        while (at < bytes.Length)
        {
            var headerEnd = bytes.AsSpan(at).IndexOf("\r\n\r\n"u8);
            Assert.True(headerEnd > 0, $"stray bytes on stdout at {at}: {Encoding.UTF8.GetString(bytes, at, Math.Min(80, bytes.Length - at))}");
            var length = -1;
            foreach (var line in Encoding.ASCII.GetString(bytes, at, headerEnd).Split("\r\n"))
            {
                var parts = line.Split(':', 2);
                Assert.Equal(2, parts.Length);
                if (parts[0].Trim().Equals("Content-Length", StringComparison.OrdinalIgnoreCase))
                {
                    length = int.Parse(parts[1].Trim(), System.Globalization.CultureInfo.InvariantCulture);
                }
            }

            Assert.True(length >= 0, "frame without Content-Length");
            var start = at + headerEnd + 4;
            messages.Add(JsonDocument.Parse(bytes.AsMemory(start, length)).RootElement.Clone());
            at = start + length;
        }

        return messages;
    }
}
