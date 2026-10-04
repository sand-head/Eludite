using System.Diagnostics;
using Eludite.Host.Build;
using Eludite.Host.Legacy;

namespace Eludite.Host.NuGet;

/// <summary>What a restore runs on and how.</summary>
/// <param name="Target">The solution or project file restored.</param>
/// <param name="Projects">The projects it covers (for the lock-file rule and the lock files reported).</param>
/// <param name="LockFiles">respect or ignore.</param>
/// <param name="Changed">A change was just written: re-evaluate (and rewrite lock files) instead of locked mode.</param>
/// <param name="Force"><c>--force-evaluate</c>.</param>
/// <param name="Environment">Variables added to the restore's environment (the session's credentials).</param>
public sealed record RestoreRequest(string Target, IReadOnlyList<string> Projects, string LockFiles, bool Changed, bool Force, IReadOnlyDictionary<string, string> Environment);

/// <summary>
/// Restores out of process (<c>dotnet restore</c>, MSBuild's Restore target; CLAUDE.md invariant 2), streaming each line
/// to a callback and reading NuGet's errors and warnings from MSBuild's canonical lines.
/// </summary>
public sealed class RestoreRunner
{
    private readonly Func<string> _dotnet;

    /// <param name="dotnet">Finds <c>dotnet</c>; default <see cref="Testing.TestContainers.LocateDotnet"/>.</param>
    public RestoreRunner(Func<string>? dotnet = null)
    {
        _dotnet = dotnet ?? Testing.TestContainers.LocateDotnet;
    }

    /// <summary>The arguments for <paramref name="request"/> and whether locked mode is used.</summary>
    public static (IReadOnlyList<string> Arguments, bool LockedMode) Arguments(RestoreRequest request)
    {
        ArgumentNullException.ThrowIfNull(request);
        var args = new List<string> { "restore", request.Target, "-nologo", "-v:m" };
        var hasLock = request.Projects.Any(p => InstalledReader.FindLockFile(p) is not null);
        var respect = !string.Equals(request.LockFiles, "ignore", StringComparison.OrdinalIgnoreCase);
        var locked = respect && hasLock && !request.Changed && !request.Force;
        if (locked)
        {
            args.Add("--locked-mode");
        }
        else if (request.Force || (respect && hasLock && request.Changed))
        {
            args.Add("--force-evaluate");
        }

        return (args, locked);
    }

    /// <summary>Runs the restore; <paramref name="line"/> gets each output line as it arrives.</summary>
    public async Task<NuGetRestoreOutcome> RunAsync(RestoreRequest request, Func<string, Task> line, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(request);
        ArgumentNullException.ThrowIfNull(line);
        var sw = Stopwatch.StartNew();
        var (args, locked) = Arguments(request);
        var dotnet = _dotnet();
        var commandLine = string.Join(' ', new[] { dotnet }.Concat(args.Select(a => a.Contains(' ', StringComparison.Ordinal) ? $"\"{a}\"" : a)));
        var psi = new ProcessStartInfo(dotnet)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            WorkingDirectory = Path.GetDirectoryName(Path.GetFullPath(request.Target)) ?? System.Environment.CurrentDirectory,
            StandardOutputEncoding = System.Text.Encoding.UTF8,
            StandardErrorEncoding = System.Text.Encoding.UTF8,
        };
        foreach (var name in DesignTimeProperties.LocatorVariables)
        {
            psi.Environment.Remove(name);
        }

        psi.Environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1";
        psi.Environment["DOTNET_NOLOGO"] = "1";
        psi.Environment["MSBUILDTERMINALLOGGER"] = "off";
        foreach (var (k, v) in request.Environment)
        {
            psi.Environment[k] = v;
        }

        foreach (var a in args)
        {
            psi.ArgumentList.Add(a);
        }

        var diagnostics = new List<NuGetDiagnostic>();
        var seen = new HashSet<string>(StringComparer.Ordinal);
        await line(commandLine).ConfigureAwait(false);
        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (System.ComponentModel.Win32Exception ex)
        {
            var message = $"Could not start {dotnet}: {ex.Message}";
            await line(message).ConfigureAwait(false);
            return new NuGetRestoreOutcome("failed", null, sw.Elapsed.TotalMilliseconds, commandLine, locked, [], [new NuGetDiagnostic("error", "ELUDITE0120", message)]);
        }

        process.StandardInput.Close();
        var gate = new SemaphoreSlim(1, 1);
        async Task Pump(StreamReader reader)
        {
            while (await reader.ReadLineAsync(CancellationToken.None).ConfigureAwait(false) is { } text)
            {
                await gate.WaitAsync(CancellationToken.None).ConfigureAwait(false);
                try
                {
                    await line(text).ConfigureAwait(false);
                    if (ConsoleLines.ParseDiagnostic(text) is { } d && seen.Add($"{d.File}|{d.Code}|{d.Message}"))
                    {
                        // NuGet writes a long message as several canonical lines with the same code: one diagnostic.
                        if (diagnostics.Count > 0 && diagnostics[^1] is var last && last.Severity == d.Severity && last.Code == d.Code && last.File == d.File && last.Project == d.Project && d.Code.Length > 0)
                        {
                            diagnostics[^1] = last with { Message = last.Message + "\n" + d.Message };
                        }
                        else
                        {
                            diagnostics.Add(new NuGetDiagnostic(d.Severity, d.Code, d.Message)
                            {
                                File = d.File,
                                Line = d.Line,
                                Column = d.Column,
                                Project = d.Project,
                            });
                        }
                    }
                }
                finally
                {
                    gate.Release();
                }
            }
        }

        var stdout = Pump(process.StandardOutput);
        var stderr = Pump(process.StandardError);
        var canceled = false;
        try
        {
            await process.WaitForExitAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            canceled = true;
            ProcessTree.Kill(process);
            using var wait = new CancellationTokenSource(TimeSpan.FromSeconds(2));
            try
            {
                await process.WaitForExitAsync(wait.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                // Reported as canceled either way.
            }
        }

        var readers = Task.WhenAll(stdout, stderr);
        await Task.WhenAny(readers, Task.Delay(TimeSpan.FromSeconds(canceled ? 0.3 : 5), CancellationToken.None)).ConfigureAwait(false);
        int? exitCode = process.HasExited ? process.ExitCode : null;
        var result = canceled ? "canceled" : exitCode == 0 ? "succeeded" : "failed";
        var lockFiles = request.Projects.Select(InstalledReader.FindLockFile).OfType<string>().ToList();
        return new NuGetRestoreOutcome(result, exitCode, Math.Round(sw.Elapsed.TotalMilliseconds, 1), commandLine, locked, lockFiles, diagnostics);
    }
}
