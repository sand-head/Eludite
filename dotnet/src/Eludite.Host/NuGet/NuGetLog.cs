using NuGet.Common;

namespace Eludite.Host.NuGet;

/// <summary>
/// NuGet's logger for the host: every message to the host's log (stderr, never stdout: CLAUDE.md invariant 10), and
/// information, warnings and errors to <paramref name="output"/> too (the Package Manager pane), when given.
/// </summary>
public sealed class NuGetLog(TextWriter log, Action<string>? output = null) : LoggerBase(LogLevel.Debug)
{
    public override void Log(ILogMessage message)
    {
        ArgumentNullException.ThrowIfNull(message);
        log.WriteLine($"[nuget] {message.Level}: {message.Message}");
        if (output is not null && message.Level >= LogLevel.Information)
        {
            output(message.Level >= LogLevel.Warning ? $"{message.Level}: {message.Message}" : message.Message);
        }
    }

    public override Task LogAsync(ILogMessage message)
    {
        Log(message);
        return Task.CompletedTask;
    }
}
