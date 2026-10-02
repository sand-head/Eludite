using System.Diagnostics;
using System.Globalization;
using System.Net.Sockets;
using System.Reflection;
using System.Text;
using Eludite.Debugger.Mono.Protocol;
using Newtonsoft.Json.Linq;

namespace Eludite.Debugger.Mono.Tests;

/// <summary>
/// The located Mono, in the order brief 0003 fixed (dotnet/src/Eludite.Host/Legacy/MonoInstallation.cs) and the shell
/// uses (crates/dap/src/discovery.rs): ELUDITE_MONO_PREFIX, mono on PATH, ~/.local/opt/mono-root/usr, /usr,
/// /usr/local, the macOS framework. Never on Windows (.NET Framework debugging there is eludite-dbg-netfx).
/// </summary>
internal static class MonoLocator
{
    public static (string Mono, IReadOnlyDictionary<string, string> Environment)? Locate()
    {
        if (OperatingSystem.IsWindows())
        {
            return null;
        }

        var candidates = new List<string>();
        if (System.Environment.GetEnvironmentVariable("ELUDITE_MONO_PREFIX") is { Length: > 0 } explicitPrefix)
        {
            candidates.Add(explicitPrefix);
        }

        foreach (var dir in (System.Environment.GetEnvironmentVariable("PATH") ?? string.Empty).Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            var mono = Path.Combine(dir, "mono");
            if (File.Exists(mono))
            {
                var real = new FileInfo(mono).ResolveLinkTarget(returnFinalTarget: true)?.FullName ?? mono;
                candidates.Add(Path.GetFullPath(Path.Combine(Path.GetDirectoryName(real)!, "..")));
            }
        }

        candidates.Add(Path.Combine(System.Environment.GetFolderPath(System.Environment.SpecialFolder.UserProfile), ".local", "opt", "mono-root", "usr"));
        candidates.Add("/usr");
        candidates.Add("/usr/local");
        candidates.Add("/Library/Frameworks/Mono.framework/Versions/Current");
        foreach (var prefix in candidates)
        {
            var mono = Path.Combine(prefix, "bin", "mono");
            if (File.Exists(mono))
            {
                return (mono, EnvironmentFor(prefix));
            }
        }

        return null;
    }

    private static Dictionary<string, string> EnvironmentFor(string prefix)
    {
        var bin = Path.Combine(prefix, "bin");
        var path = System.Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        var env = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["PATH"] = path.Split(Path.PathSeparator).Contains(bin) ? path : bin + Path.PathSeparator + path,
        };
        if (!string.Equals(Path.GetFullPath(prefix).TrimEnd('/'), "/usr", StringComparison.Ordinal))
        {
            var lib = Path.Combine(prefix, "lib");
            var ld = System.Environment.GetEnvironmentVariable("LD_LIBRARY_PATH");
            env["LD_LIBRARY_PATH"] = string.IsNullOrEmpty(ld) ? lib : lib + Path.PathSeparator + ld;
            env["MONO_CFG_DIR"] = Path.GetFullPath(Path.Combine(prefix, "..", "etc"));
            env["MONO_GAC_PREFIX"] = prefix;
        }

        return env;
    }
}

/// <summary>The built adapter and TestApp (paths baked in at build time for this configuration).</summary>
internal static class Built
{
    private static string Metadata(string key) =>
        typeof(Built).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>().Single(a => a.Key == key).Value
        ?? throw new InvalidOperationException(key);

    public static string Adapter => Path.GetFullPath(Metadata("AdapterPath"));

    public static string TestApp => Path.GetFullPath(Metadata("TestAppPath"));

    public static string TestAppSource => Path.GetFullPath(Metadata("TestAppSource"));

    private static readonly Lazy<string[]> SourceLines = new(() => File.ReadAllLines(TestAppSource));

    /// <summary>The 1-based line of the TestApp's <c>// MARK: name</c> comment.</summary>
    public static int Line(string mark)
    {
        var lines = SourceLines.Value;
        for (var i = 0; i < lines.Length; i++)
        {
            if (lines[i].EndsWith("// MARK: " + mark, StringComparison.Ordinal))
            {
                return i + 1;
            }
        }

        throw new InvalidOperationException("no MARK: " + mark + " in " + TestAppSource);
    }
}

/// <summary>A DAP client for the tests: requests wait for their answers, events are recorded.</summary>
internal sealed class DapTestClient : IDisposable
{
    public static readonly TimeSpan Timeout = TimeSpan.FromSeconds(20);

    private readonly Process _process;
    private readonly Stream _writer;
    private readonly TcpClient? _tcp;
    private readonly object _lock = new();
    private readonly Dictionary<int, JObject> _responses = new();
    private readonly List<JObject> _events = new();
    private readonly StringBuilder _stderr = new();
    private int _seq;

    private DapClientState State { get; } = new();

    private DapTestClient(Process process, Stream reader, Stream writer, TcpClient? tcp)
    {
        _process = process;
        _writer = writer;
        _tcp = tcp;
        var frames = new FrameReader(reader);
        var thread = new Thread(() =>
        {
            try
            {
                while (frames.Read() is { } m)
                {
                    lock (_lock)
                    {
                        if ((string?)m["type"] == "response")
                        {
                            _responses[(int)m["request_seq"]!] = m;
                        }
                        else
                        {
                            _events.Add(m);
                        }

                        Monitor.PulseAll(_lock);
                    }
                }
            }
            catch (IOException)
            {
            }
            catch (ObjectDisposedException)
            {
            }

            lock (_lock)
            {
                State.Closed = true;
                Monitor.PulseAll(_lock);
            }
        })
        { IsBackground = true };
        thread.Start();
    }

    /// <summary>The adapter process (its pid for the memory reading).</summary>
    public Process Process => _process;

    public string Stderr
    {
        get
        {
            lock (_stderr)
            {
                return _stderr.ToString();
            }
        }
    }

    private static Process StartAdapter(string mono, IReadOnlyDictionary<string, string> env, params string[] options)
    {
        var info = new ProcessStartInfo(mono)
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        info.ArgumentList.Add(Built.Adapter);
        foreach (var o in options)
        {
            info.ArgumentList.Add(o);
        }

        foreach (var kv in env)
        {
            info.Environment[kv.Key] = kv.Value;
        }

        return Process.Start(info) ?? throw new InvalidOperationException("cannot start " + mono);
    }

    /// <summary><c>mono eludite-dbg-mono.exe</c> over stdio.</summary>
    public static DapTestClient Stdio(string mono, IReadOnlyDictionary<string, string> env)
    {
        var p = StartAdapter(mono, env);
        var c = new DapTestClient(p, p.StandardOutput.BaseStream, p.StandardInput.BaseStream, null);
        c.DrainStderr(p.StandardError);
        return c;
    }

    /// <summary><c>mono eludite-dbg-mono.exe --port 0</c>, connected over TCP to the port it prints.</summary>
    public static DapTestClient Tcp(string mono, IReadOnlyDictionary<string, string> env)
    {
        var p = StartAdapter(mono, env, "--port", "0");
        var line = p.StandardError.ReadLine() ?? throw new InvalidOperationException("the adapter printed no port");
        const string Prefix = "eludite-dbg-mono: listening on 127.0.0.1:";
        Assert.StartsWith(Prefix, line, StringComparison.Ordinal);
        var port = int.Parse(line.AsSpan(Prefix.Length), CultureInfo.InvariantCulture);
        var tcp = new TcpClient();
        tcp.Connect("127.0.0.1", port);
        var stream = tcp.GetStream();
        var c = new DapTestClient(p, stream, stream, tcp);
        c.DrainStderr(p.StandardError);
        return c;
    }

    private void DrainStderr(StreamReader stderr)
    {
        var t = new Thread(() =>
        {
            while (stderr.ReadLine() is { } l)
            {
                lock (_stderr)
                {
                    _stderr.AppendLine(l);
                }
            }
        })
        { IsBackground = true };
        t.Start();
    }

    /// <summary>Send a request and wait for its response (successful or not).</summary>
    public JObject Request(string command, JObject? arguments = null, TimeSpan? timeout = null)
    {
        int seq;
        lock (_lock)
        {
            seq = ++_seq;
        }

        var message = new JObject { ["seq"] = seq, ["type"] = "request", ["command"] = command };
        if (arguments is not null)
        {
            message["arguments"] = arguments;
        }

        var bytes = Framing.Encode(message);
        _writer.Write(bytes, 0, bytes.Length);
        _writer.Flush();
        var deadline = DateTime.UtcNow + (timeout ?? Timeout);
        lock (_lock)
        {
            while (!_responses.ContainsKey(seq))
            {
                var left = deadline - DateTime.UtcNow;
                if (left <= TimeSpan.Zero || State.Closed)
                {
                    throw new TimeoutException("no answer to " + command + (State.Closed ? " (the adapter closed the channel)" : string.Empty) + "\n" + Stderr);
                }

                Monitor.Wait(_lock, left);
            }

            return _responses[seq];
        }
    }

    /// <summary>Send a request that must succeed and answer its body (an empty object when it has none).</summary>
    public JObject Body(string command, JObject? arguments = null, TimeSpan? timeout = null)
    {
        var r = Request(command, arguments, timeout);
        Assert.True((bool?)r["success"] == true, command + " failed: " + r["message"] + "\n" + Stderr);
        return r["body"] as JObject ?? new JObject();
    }

    /// <summary>The <paramref name="nth"/> (1-based) event named <paramref name="name"/> matching <paramref name="match"/>.</summary>
    public JObject WaitEvent(string name, int nth = 1, Func<JObject, bool>? match = null, TimeSpan? timeout = null)
    {
        var deadline = DateTime.UtcNow + (timeout ?? Timeout);
        lock (_lock)
        {
            while (true)
            {
                var found = _events.Where(e => (string?)e["event"] == name && (match is null || match(e))).Skip(nth - 1).FirstOrDefault();
                if (found is not null)
                {
                    return found;
                }

                var left = deadline - DateTime.UtcNow;
                if (left <= TimeSpan.Zero || State.Closed)
                {
                    throw new TimeoutException("no event " + name + " #" + nth + "; events: " + string.Join(", ", _events.Select(e => (string?)e["event"])) + "\n" + Stderr);
                }

                Monitor.Wait(_lock, left);
            }
        }
    }

    /// <summary>The events named <paramref name="name"/> so far.</summary>
    public List<JObject> Events(string name)
    {
        lock (_lock)
        {
            return _events.Where(e => (string?)e["event"] == name).ToList();
        }
    }

    /// <summary>Wait until the adapter process ends.</summary>
    public bool WaitForExit(TimeSpan timeout) => _process.WaitForExit(timeout);

    public void Dispose()
    {
        try
        {
            if (!_process.HasExited)
            {
                _process.Kill(entireProcessTree: true);
            }
        }
        catch (InvalidOperationException)
        {
        }

        _tcp?.Dispose();
        _process.Dispose();
    }

    private sealed class DapClientState
    {
        public bool Closed { get; set; }
    }
}
