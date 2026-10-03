using System.ComponentModel;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;

namespace Eludite.TestBridge;

/// <summary>A runner's child process: its output lines go to the sink's log, and it is killed with its children.</summary>
internal sealed class TestProcess : IDisposable
{
    private readonly Process _process;

    private TestProcess(Process process) => _process = process;

    public int Id => _process.Id;

    public Task Exited => _process.WaitForExitAsync();

    public int? ExitCode
    {
        get
        {
            try
            {
                return _process.HasExited ? _process.ExitCode : null;
            }
            catch (InvalidOperationException)
            {
                return null;
            }
        }
    }

    /// <summary>The variables every runner gets: no telemetry (CLAUDE.md), no logo, no first-run work.</summary>
    public static IReadOnlyDictionary<string, string> QuietEnvironment { get; } = new Dictionary<string, string>(StringComparer.Ordinal)
    {
        ["TESTINGPLATFORM_TELEMETRY_OPTOUT"] = "1",
        ["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1",
        ["DOTNET_NOLOGO"] = "1",
        ["DOTNET_SKIP_FIRST_TIME_EXPERIENCE"] = "1",
        ["TESTINGPLATFORM_NOBANNER"] = "1",
    };

    public static TestProcess Start(string fileName, IEnumerable<string> args, string cwd, IReadOnlyDictionary<string, string>? env, Action<string> log)
    {
        var psi = new ProcessStartInfo(fileName)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true,
            WorkingDirectory = cwd,
            StandardOutputEncoding = System.Text.Encoding.UTF8,
            StandardErrorEncoding = System.Text.Encoding.UTF8,
        };
        foreach (var a in args)
        {
            psi.ArgumentList.Add(a);
        }

        foreach (var (k, v) in QuietEnvironment)
        {
            psi.Environment[k] = v;
        }

        if (env is not null)
        {
            foreach (var (k, v) in env)
            {
                psi.Environment[k] = v;
            }
        }

        Process process;
        try
        {
            process = Process.Start(psi) ?? throw new TestRunnerException($"{fileName} did not start");
        }
        catch (Win32Exception ex)
        {
            throw new TestRunnerException($"{fileName} could not start: {ex.Message}", ex);
        }

        process.OutputDataReceived += (_, e) =>
        {
            if (e.Data is { Length: > 0 } line)
            {
                log(line);
            }
        };
        process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data is { Length: > 0 } line)
            {
                log(line);
            }
        };
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();
        return new TestProcess(process);
    }

    /// <summary>The command line as a person would type it (for the Tests output).</summary>
    public static string CommandLine(string fileName, IEnumerable<string> args) =>
        string.Join(' ', new[] { fileName }.Concat(args).Select(a => a.Contains(' ', StringComparison.Ordinal) ? $"\"{a}\"" : a));

    /// <summary>Kills the process and everything it started.</summary>
    public void Kill()
    {
        try
        {
            if (!_process.HasExited)
            {
                _process.Kill(entireProcessTree: true);
            }
        }
        catch (Exception ex) when (ex is InvalidOperationException or Win32Exception or AggregateException)
        {
        }
    }

    public void Dispose()
    {
        Kill();
        _process.Dispose();
    }

    /// <summary>A loopback listener on a free port.</summary>
    public static TcpListener Listen()
    {
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start(1);
        return listener;
    }

    public static int Port(TcpListener listener) => ((IPEndPoint)listener.LocalEndpoint).Port;

    /// <summary>
    /// Waits for the runner to connect, failing when the process exits first or <paramref name="timeout"/> passes.
    /// </summary>
    public static async Task<TcpClient> AcceptAsync(TcpListener listener, TestProcess? process, TimeSpan timeout, string what, CancellationToken cancellationToken)
    {
        using var timer = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timer.CancelAfter(timeout);
        var accept = listener.AcceptTcpClientAsync(timer.Token).AsTask();
        var exited = process?.Exited ?? Task.Delay(Timeout.Infinite, timer.Token);
        try
        {
            var first = await Task.WhenAny(accept, exited).ConfigureAwait(false);
            if (first == accept)
            {
                var client = await accept.ConfigureAwait(false);
                client.NoDelay = true;
                return client;
            }
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            throw new TestRunnerException($"{what} did not connect within {timeout.TotalSeconds:0} s");
        }

        cancellationToken.ThrowIfCancellationRequested();
        if (process is not null && process.ExitCode is { } code)
        {
            throw new TestRunnerException($"{what} exited with code {code} before it connected");
        }

        throw new TestRunnerException($"{what} did not connect within {timeout.TotalSeconds:0} s");
    }
}
