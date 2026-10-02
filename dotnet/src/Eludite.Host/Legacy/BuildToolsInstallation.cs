using System.Diagnostics;

namespace Eludite.Host.Legacy;

/// <summary>
/// MSBuild.exe from Visual Studio Build Tools (or any VS edition), located with <c>vswhere</c>. Never bundled
/// (CLAUDE.md invariant 9). Windows only; untested in brief 0003 (no Windows machine was available).
/// </summary>
public sealed record BuildToolsInstallation(string MsBuildExe, string Source)
{
    /// <summary>Runs <c>vswhere -latest -products * -requires Microsoft.Component.MSBuild -find MSBuild\**\Bin\MSBuild.exe</c>.</summary>
    public static BuildToolsInstallation? Locate()
    {
        if (!OperatingSystem.IsWindows())
        {
            return null;
        }

        if (Environment.GetEnvironmentVariable("ELUDITE_MSBUILD_EXE") is { Length: > 0 } explicitExe && File.Exists(explicitExe))
        {
            return new BuildToolsInstallation(explicitExe, "ELUDITE_MSBUILD_EXE");
        }

        var programFilesX86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
        var vswhere = Path.Combine(programFilesX86, "Microsoft Visual Studio", "Installer", "vswhere.exe");
        if (!File.Exists(vswhere))
        {
            return null;
        }

        var psi = new ProcessStartInfo(vswhere)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var arg in new[] { "-latest", "-products", "*", "-requires", "Microsoft.Component.MSBuild", "-find", @"MSBuild\**\Bin\MSBuild.exe" })
        {
            psi.ArgumentList.Add(arg);
        }

        using var process = Process.Start(psi);
        if (process is null)
        {
            return null;
        }

        var first = process.StandardOutput.ReadToEnd()
            .Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .FirstOrDefault(File.Exists);
        process.WaitForExit();
        return first is null ? null : new BuildToolsInstallation(first, "vswhere");
    }
}
