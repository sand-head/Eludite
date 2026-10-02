using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;

namespace Eludite.Host.Build;

/// <summary>Kills a process and everything it started (the MSBuild entry process and its worker nodes, Exec'd tools).</summary>
public static class ProcessTree
{
    /// <summary>
    /// Linux and macOS: <see cref="Process.Kill(bool)"/> with the whole tree, which walks the children (<c>/proc</c>,
    /// <c>sysctl</c>) and kills them. Windows: <c>taskkill /T /F /PID</c>, then the same call for what is left
    /// (untested: no Windows machine in brief 0017; a Job Object with KILL_ON_JOB_CLOSE is the sturdier follow-up).
    /// </summary>
    public static void Kill(Process process, TextWriter? log = null)
    {
        ArgumentNullException.ThrowIfNull(process);
        try
        {
            if (process.HasExited)
            {
                return;
            }
        }
        catch (InvalidOperationException)
        {
            return;
        }

        if (OperatingSystem.IsWindows())
        {
            try
            {
                using var taskkill = Process.Start(new ProcessStartInfo("taskkill", ["/T", "/F", "/PID", process.Id.ToString(CultureInfo.InvariantCulture)])
                {
                    UseShellExecute = false,
                    CreateNoWindow = true,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                });
                taskkill?.WaitForExit(2000);
            }
            catch (Win32Exception ex)
            {
                log?.WriteLine($"[build] taskkill failed: {ex.Message}");
            }
        }

        try
        {
            process.Kill(entireProcessTree: true);
        }
        catch (Exception ex) when (ex is InvalidOperationException or Win32Exception or AggregateException)
        {
            log?.WriteLine($"[build] kill: {ex.Message}");
        }
    }
}
