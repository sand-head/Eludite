using System.Diagnostics;
using Eludite.Host.Lsp;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0080: where the host finds the Roslyn language server. The argument (the shell's setting
/// <c>languageServers.roslynPath</c>), then <c>ELUDITE_ROSLYN_LS</c>, then <c>roslyn/</c> beside the host (the package
/// layout), then tools/roslyn-pin's output; a named path that does not exist is not silently replaced.
/// </summary>
public sealed class RoslynLocateTests : IDisposable
{
    private readonly string _root = Directory.CreateTempSubdirectory("eludite-roslyn-locate-").FullName;

    public void Dispose() => TestDirectory.Delete(_root);

    private string Server(string folder)
    {
        var dir = Path.Combine(_root, folder);
        Directory.CreateDirectory(dir);
        var dll = Path.Combine(dir, RoslynProcessLauncher.ServerDll);
        File.WriteAllText(dll, "");
        return dll;
    }

    private string CacheServer()
    {
        var dir = Path.Combine(_root, "src", "artifacts", "bin", "Microsoft.CodeAnalysis.LanguageServer", "Release", "net10.0");
        Directory.CreateDirectory(dir);
        var dll = Path.Combine(dir, RoslynProcessLauncher.ServerDll);
        File.WriteAllText(dll, "");
        return dll;
    }

    private static Func<string, string?> Env(params (string Name, string Value)[] vars) =>
        name => vars.FirstOrDefault(v => v.Name == name).Value;

    [Fact]
    public void Order_IsArgument_Environment_BesideHost_Cache()
    {
        var argument = Server("setting");
        var fromEnv = Server("env");
        var host = Path.Combine(_root, "host");
        var bundled = Server(Path.Combine("host", RoslynProcessLauncher.BundledFolder));
        var cache = CacheServer();
        var env = Env(("ELUDITE_ROSLYN_LS", fromEnv), ("ROSLYN_SRC_DIR", Path.Combine(_root, "src")));

        Assert.Equal(argument, RoslynProcessLauncher.Locate(argument, host, env, out var source));
        Assert.Equal(RoslynProcessLauncher.Source.Argument, source);

        Assert.Equal(fromEnv, RoslynProcessLauncher.Locate(null, host, env, out source));
        Assert.Equal(RoslynProcessLauncher.Source.Environment, source);

        var noEnv = Env(("ROSLYN_SRC_DIR", Path.Combine(_root, "src")));
        Assert.Equal(bundled, RoslynProcessLauncher.Locate(null, host, noEnv, out source));
        Assert.Equal(RoslynProcessLauncher.Source.BesideHost, source);

        Assert.Equal(cache, RoslynProcessLauncher.Locate(null, Path.Combine(_root, "elsewhere"), noEnv, out source));
        Assert.Equal(RoslynProcessLauncher.Source.Cache, source);
    }

    [Fact]
    public void A_NamedPathThatDoesNotExist_IsNotReplacedByTheShippedServer()
    {
        var host = Path.Combine(_root, "host");
        Server(Path.Combine("host", RoslynProcessLauncher.BundledFolder));
        var missing = Path.Combine(_root, "nope", RoslynProcessLauncher.ServerDll);

        Assert.Null(RoslynProcessLauncher.Locate(missing, host, Env(), out var source));
        Assert.Equal(RoslynProcessLauncher.Source.Argument, source);

        Assert.Null(RoslynProcessLauncher.Locate(null, host, Env(("ELUDITE_ROSLYN_LS", missing)), out source));
        Assert.Equal(RoslynProcessLauncher.Source.Environment, source);
    }

    [Fact]
    public void NothingAnywhere_IsNull_AndTheMessageNamesTheSettingAndTheDocs()
    {
        Assert.Null(RoslynProcessLauncher.Locate(null, Path.Combine(_root, "host"), Env(("ROSLYN_SRC_DIR", Path.Combine(_root, "src"))), out var source));
        Assert.Equal(RoslynProcessLauncher.Source.Cache, source);
        Assert.Contains("languageServers.roslynPath", RoslynProcessLauncher.NotFoundMessage, StringComparison.Ordinal);
        Assert.Contains("docs/user/troubleshooting.md", RoslynProcessLauncher.NotFoundMessage, StringComparison.Ordinal);
        Assert.DoesNotContain("tools/roslyn-pin", RoslynProcessLauncher.NotFoundMessage, StringComparison.Ordinal);
    }

    [Fact]
    public async Task Version_PrintsTheHostVersionOnStdout()
    {
        var psi = new ProcessStartInfo("dotnet")
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        psi.ArgumentList.Add(Path.Combine(AppContext.BaseDirectory, "eludite-host.dll"));
        psi.ArgumentList.Add("--version");
        using var host = Process.Start(psi)!;
        var stdout = await host.StandardOutput.ReadToEndAsync(TestContext.Current.CancellationToken);
        await host.WaitForExitAsync(TestContext.Current.CancellationToken);
        Assert.Equal(0, host.ExitCode);
        Assert.StartsWith("eludite-host ", stdout, StringComparison.Ordinal);
        Assert.Contains(Eludite.Host.Rpc.HostRpcTarget.HostVersion, stdout, StringComparison.Ordinal);
    }
}
