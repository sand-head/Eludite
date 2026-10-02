using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Text;
using Eludite.Debugger.Mono.Protocol;
using Mono.Debugger.Soft;
using Mono.Debugging.Client;

namespace Eludite.Debugger.Mono;

/// <summary>
/// <c>mono eludite-dbg-mono.exe [--port N] [--log FILE]</c>: DAP on stdio, or on TCP for one client
/// (protocol/schemas/dap-mono.md). stdout carries DAP only (CLAUDE.md invariant 10): anything else that writes to
/// <see cref="Console.Out"/> is sent to stderr.
/// </summary>
internal static class Program
{
    public static int Main(string[] args)
    {
        AdapterOptions options;
        try
        {
            options = AdapterOptions.Parse(args);
        }
        catch (DapException e)
        {
            Console.Error.WriteLine("eludite-dbg-mono: " + e.Message);
            Console.Error.WriteLine(AdapterOptions.Usage);
            return 2;
        }

        if (options.Help)
        {
            Console.Error.WriteLine(AdapterOptions.Usage);
            return 0;
        }

        var stdout = Console.OpenStandardOutput();
        Console.SetOut(Console.Error);
        using var log = new Log(options.LogFile);
        DebuggerLoggingService.CustomLogger = log;

        Stream input;
        Stream output;
        TcpClient? client = null;
        if (options.Port is int port)
        {
            var listener = new TcpListener(IPAddress.Loopback, port);
            listener.Start();
            var bound = ((IPEndPoint)listener.LocalEndpoint).Port;
            Console.Error.WriteLine("eludite-dbg-mono: listening on 127.0.0.1:" + bound.ToString(CultureInfo.InvariantCulture));
            Console.Error.Flush();
            client = listener.AcceptTcpClient();
            listener.Stop();
            client.NoDelay = true;
            input = output = client.GetStream();
            log.Write("client connected over TCP");
        }
        else
        {
            input = Console.OpenStandardInput();
            output = stdout;
        }

        using (var dispatcher = new Dispatcher(input, output, log.Write))
        {
            var adapter = new MonoAdapter(dispatcher, log);
            dispatcher.Run();
            adapter.Shutdown();
        }

        client?.Close();
        log.Write("exiting");
        // Mono.Debugging leaves foreground threads behind; the session is over.
        Environment.Exit(0);
        return 0;
    }
}

/// <summary>The adapter's log: stderr, and a file with <c>--log</c>. Also Mono.Debugging's logger, which must be set before a session runs.</summary>
internal sealed class Log : ICustomLogger, IDisposable
{
    private readonly object _lock = new();
    private readonly StreamWriter? _file;
    private readonly Stopwatch _clock = Stopwatch.StartNew();

    public Log(string? path)
    {
        if (!string.IsNullOrEmpty(path))
        {
            _file = new StreamWriter(path!, append: true, new UTF8Encoding(false)) { AutoFlush = true };
        }
    }

    public void Write(string message)
    {
        var line = "[eludite-dbg-mono " + _clock.ElapsedMilliseconds.ToString(CultureInfo.InvariantCulture) + " ms] " + message;
        lock (_lock)
        {
            Console.Error.WriteLine(line);
            _file?.WriteLine(line);
        }
    }

    public string GetNewDebuggerLogFilename() => null!;

    public void LogAndShowException(string message, Exception ex) => Write(message + ": " + ex);

    public void LogError(string message, Exception ex) => Write(message + (ex is null ? string.Empty : ": " + ex));

    public void LogMessage(string messageFormat, params object[] args) =>
        Write(args is { Length: > 0 } ? string.Format(CultureInfo.InvariantCulture, messageFormat, args) : messageFormat);

    public void Dispose() => _file?.Dispose();
}

/// <summary>
/// The debuggee, started by the adapter rather than by Mono.Debugger.Soft so its stdin can be closed (the adapter's
/// stdin is the DAP channel) and its process id is known. stdout and stderr stay redirected for the session to read.
/// </summary>
internal sealed class TargetProcess : ITargetProcess
{
    private readonly Process _process;

    public TargetProcess(ProcessStartInfo info)
    {
        info.UseShellExecute = false;
        info.RedirectStandardOutput = true;
        info.RedirectStandardError = true;
        info.RedirectStandardInput = true;
        info.StandardOutputEncoding = Encoding.UTF8;
        info.StandardErrorEncoding = Encoding.UTF8;
        _process = new Process { StartInfo = info, EnableRaisingEvents = true };
        _process.Exited += (s, e) => Exited?.Invoke(this, e);
        _process.Start();
        _process.StandardInput.Close();
    }

    public event EventHandler? Exited;

    public bool HasExited => _process.HasExited;

    public int Id => _process.Id;

    public string ProcessName => Path.GetFileName(_process.StartInfo.FileName);

    public StreamReader StandardError => _process.StandardError;

    public StreamReader StandardOutput => _process.StandardOutput;

    /// <summary>The exit code once it exited.</summary>
    public int? ExitCode => _process.HasExited ? _process.ExitCode : null;

    public bool WaitForExit(int milliseconds) => _process.WaitForExit(milliseconds);

    public void Kill()
    {
        try
        {
            if (!_process.HasExited)
            {
                _process.Kill();
            }
        }
        catch (InvalidOperationException)
        {
            // Exited meanwhile.
        }
    }
}
