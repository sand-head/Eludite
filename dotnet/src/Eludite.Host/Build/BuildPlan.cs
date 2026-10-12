using System.Text;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;

namespace Eludite.Host.Build;

/// <summary>The MSBuilds the host can build with, located once (brief 0003: located, never bundled).</summary>
public sealed record BuildToolchains(MonoInstallation? Mono, BuildToolsInstallation? BuildTools, string? TargetFrameworkRootPath)
{
    /// <summary>Mono (unless <c>ELUDITE_LEGACY_MONO=0</c>), Build Tools on Windows, the reference-assembly root.</summary>
    public static BuildToolchains Locate()
    {
        var mono = Environment.GetEnvironmentVariable("ELUDITE_LEGACY_MONO") == "0" ? null : MonoInstallation.Locate();
        BuildToolsInstallation? tools = null;
        try
        {
            tools = BuildToolsInstallation.Locate();
        }
        catch (System.ComponentModel.Win32Exception)
        {
        }

        string? root = null;
        try
        {
            root = new ReferenceAssemblies().MergedRoot(LegacyDesignTime.DefaultCacheDirectory());
        }
        catch (IOException)
        {
        }

        return new BuildToolchains(mono, tools, root);
    }
}

/// <summary>How to run one build: the program, its arguments and environment, and what the reply reports.</summary>
public sealed record BuildPlan(BuildToolchain Toolchain, string FileName, IReadOnlyList<string> Arguments, IReadOnlyDictionary<string, string> Environment, IReadOnlyList<BuildDiagnostic> Notes)
{
    /// <summary>
    /// <c>dotnet build</c> (<c>-t:Rebuild</c>; <c>dotnet clean</c>) for SDK-style solutions. For a solution with legacy
    /// projects: Build Tools' MSBuild on Windows, else Mono's, else <c>dotnet build</c> with an
    /// <see cref="WindowsOnlyTargets.NoLegacyMsBuild"/> note. <c>publish</c> (brief 0074): <c>dotnet publish</c> of one
    /// SDK-style project with <c>-p:PublishProfile=</c>; a solution or a legacy project is refused (-32602).
    /// </summary>
    /// <param name="configuration">Passed as <c>-p:Configuration=</c> when not null (a publish profile's own applies otherwise).</param>
    public static BuildPlan Create(string target, string path, string? configuration, string? platform, string? binlog, bool hasLegacy, BuildToolchains toolchains, string? publishProfile = null)
    {
        ArgumentNullException.ThrowIfNull(toolchains);
        ArgumentNullException.ThrowIfNull(path);
        if (target == BuildTargets.Publish)
        {
            if (!SolutionProjects.IsProjectFile(path))
            {
                throw HostErrors.BadParams("publish needs a project");
            }

            if (hasLegacy)
            {
                throw HostErrors.BadParams($"{path} is a legacy (non-SDK) project: Eludite publishes SDK-style projects only");
            }
        }

        var common = new List<string> { "-nologo", "-v:m", "-nr:false", "-clp:ForceNoAlign" };
        if (configuration is not null)
        {
            common.Add($"-p:Configuration={configuration}");
        }
        if (platform is not null)
        {
            common.Add($"-p:Platform={platform}");
        }

        if (binlog is not null)
        {
            common.Add($"-bl:{binlog}");
        }

        var env = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["MSBUILDDISABLENODEREUSE"] = "1",
            ["MSBUILDTERMINALLOGGER"] = "off",
            ["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1",
            ["DOTNET_NOLOGO"] = "1",
            ["DOTNET_SKIP_FIRST_TIME_EXPERIENCE"] = "1",
        };
        var msbuildTarget = target switch
        {
            BuildTargets.Rebuild => "Rebuild",
            BuildTargets.Clean => "Clean",
            _ => "Build",
        };

        if (hasLegacy && (toolchains.BuildTools is not null || toolchains.Mono is not null))
        {
            var command = toolchains.BuildTools is { } tools ? MsBuildCommand.ForBuildTools(tools) : MsBuildCommand.ForMono(toolchains.Mono!);
            foreach (var (k, v) in command.Environment)
            {
                env[k] = v;
            }

            if (toolchains.TargetFrameworkRootPath is { } root)
            {
                env["TargetFrameworkRootPath"] = root;
            }

            var args = new List<string>(command.PrefixArguments) { path, "-restore", $"-t:{msbuildTarget}", "-m" };
            args.AddRange(common);
            var toolchain = toolchains.BuildTools is { } t
                ? new BuildToolchain("buildTools") { Path = t.MsBuildExe, Source = t.Source }
                : new BuildToolchain("mono") { Path = toolchains.Mono!.MsBuildDll, Source = toolchains.Mono.Source };
            return new BuildPlan(toolchain, command.FileName, args, env, []);
        }

        var dotnetArgs = target switch
        {
            BuildTargets.Clean => new List<string> { "clean", path },
            BuildTargets.Publish => new List<string> { "publish", path },
            _ => new List<string> { "build", path },
        };
        if (target == BuildTargets.Rebuild)
        {
            dotnetArgs.Add("-t:Rebuild");
        }

        dotnetArgs.Add("-tl:off");
        dotnetArgs.AddRange(common);
        if (target == BuildTargets.Publish && publishProfile is { Length: > 0 })
        {
            dotnetArgs.Add($"-p:PublishProfile={publishProfile}");
        }
        var notes = new List<BuildDiagnostic>();
        if (hasLegacy)
        {
            notes.Add(new BuildDiagnostic("warning", WindowsOnlyTargets.NoLegacyMsBuild, OperatingSystem.IsWindows()
                ? "The solution has legacy (non-SDK) projects and no Visual Studio Build Tools MSBuild was found; building with `dotnet build`, which cannot build most of them. Install Build Tools."
                : "The solution has legacy (non-SDK) projects and no Mono MSBuild was found; building with `dotnet build`, which cannot build most of them. Install Mono and mono-msbuild.")
            {
                File = path,
            });
        }

        return new BuildPlan(new BuildToolchain("dotnet") { Path = "dotnet" }, "dotnet", dotnetArgs, env, notes);
    }

    /// <summary>The command line as the Output window shows it (arguments with spaces quoted).</summary>
    public string CommandLine
    {
        get
        {
            var sb = new StringBuilder(Quote(FileName));
            foreach (var a in Arguments)
            {
                sb.Append(' ').Append(Quote(a));
            }

            return sb.ToString();
        }
    }

    private static string Quote(string s) => s.Contains(' ', StringComparison.Ordinal) ? $"\"{s}\"" : s;
}
