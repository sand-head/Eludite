using System.Diagnostics;
using System.Text.Json;
using Niello.Host.Legacy;
using Niello.Host.Lsp;
using Niello.Host.Rpc;

namespace Niello.Host.Tests;

/// <summary>
/// Brief 0003 proving test: WebForms code-behind IntelliSense for a field declared only in markup. Drives the real
/// niello-host process with the Roslyn language server against a copy of a corpus WebForms project
/// (corpus/legacy, aspnet/samples ChangePK). The copy gets one extra <c>asp:Button</c> in markup (absent from the
/// checked-in .designer.cs) and one control whose tag prefix is registered only in web.config
/// <c>&lt;pages&gt;&lt;controls&gt;</c>. Skips with a message when Roslyn or the corpus checkout is absent.
/// </summary>
public sealed class LegacyWebFormsCompletionTests
{
    private const string ProbeButton = "NielloProbeButton";
    private const string ProbeLabel = "NielloProbeLabel";

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Theory]
    [InlineData("mono")]
    [InlineData("sdk")]
    public async Task CodeBehind_CompletionOnMarkupOnlyField_ListsControlMembers(string msbuild)
    {
        var roslyn = RoslynProcessLauncher.Locate(null);
        Assert.SkipWhen(roslyn is null, "Roslyn language server not built; run tools/roslyn-pin/build.sh (or set NIELLO_ROSLYN_LS).");
        var source = FindCorpusProject();
        Assert.SkipWhen(source is null, "Corpus checkout not found; run corpus/legacy/fetch.sh (or set NIELLO_LEGACY_CORPUS).");
        Assert.SkipWhen(msbuild == "mono" && (OperatingSystem.IsWindows() || MonoInstallation.Locate() is null), "Mono MSBuild not found; see tools/legacy-load/README.md.");
        Assert.SkipWhen(new ReferenceAssemblies().RootFor("v4.5") is null, "Microsoft.NETFramework.ReferenceAssemblies.net45 is not in the NuGet cache; run tools/legacy-load/run.sh --prepare-only.");

        var work = Directory.CreateTempSubdirectory("niello-0003-");
        try
        {
            var project = PrepareCopy(source!, work.FullName);
            var codeBehind = Path.Combine(Path.GetDirectoryName(project)!, "Account", "Login.aspx.cs");
            await RunAsync(roslyn!, project, codeBehind, Path.Combine(work.FullName, "cache"), msbuild);
        }
        finally
        {
            try
            {
                work.Delete(recursive: true);
            }
            catch (IOException)
            {
            }
        }
    }

    private static string PrepareCopy(string sourceProjectDir, string work)
    {
        var dest = Path.Combine(work, "PrimaryKeysConfigTest");
        foreach (var file in Directory.EnumerateFiles(sourceProjectDir, "*", SearchOption.AllDirectories))
        {
            var rel = Path.GetRelativePath(sourceProjectDir, file);
            if (rel.StartsWith("bin", StringComparison.Ordinal) || rel.StartsWith("obj", StringComparison.Ordinal))
            {
                continue;
            }

            var target = Path.Combine(dest, rel);
            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            File.Copy(file, target);
        }

        // Markup-only controls: give the "Log in" button an ID, and add a label whose tag prefix only web.config knows.
        var login = Path.Combine(dest, "Account", "Login.aspx");
        var markup = File.ReadAllText(login);
        const string button = """<asp:Button runat="server" OnClick="LogIn" """;
        Assert.Contains(button, markup, StringComparison.Ordinal);
        markup = markup.Replace(button, $"""<asp:Button runat="server" ID="{ProbeButton}" OnClick="LogIn" """, StringComparison.Ordinal);
        markup = markup.Replace("</asp:Content>", $"""    <niello:Label runat="server" ID="{ProbeLabel}" />{Environment.NewLine}</asp:Content>""", StringComparison.Ordinal);
        File.WriteAllText(login, markup);
        Assert.DoesNotContain(ProbeButton, File.ReadAllText(login + ".designer.cs"), StringComparison.Ordinal);

        var webConfig = Path.Combine(dest, "Web.config");
        var config = File.ReadAllText(webConfig);
        const string controls = "<controls>";
        Assert.Contains(controls, config, StringComparison.Ordinal);
        File.WriteAllText(webConfig, config.Replace(controls, controls + """<add tagPrefix="niello" namespace="System.Web.UI.WebControls" assembly="System.Web" />""", StringComparison.Ordinal));
        return Path.Combine(dest, "PrimaryKeysConfigTest.csproj");
    }

    private static async Task RunAsync(string roslyn, string project, string codeBehind, string cacheDir, string msbuild)
    {
        var psi = new ProcessStartInfo("dotnet")
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        psi.ArgumentList.Add(Path.Combine(AppContext.BaseDirectory, "niello-host.dll"));
        psi.ArgumentList.Add("--roslyn-ls");
        psi.ArgumentList.Add(roslyn);
        psi.Environment["NIELLO_CACHE_DIR"] = cacheDir;
        psi.Environment["NIELLO_LEGACY_MONO"] = msbuild == "mono" ? "1" : "0";
        psi.Environment.Remove("NIELLO_LEGACY_DESIGNERS");
        foreach (var name in DesignTimeProperties.LocatorVariables)
        {
            psi.Environment.Remove(name);
        }
        using var host = Process.Start(psi)!;
        var stderr = host.StandardError.ReadToEndAsync(Ct);
        try
        {
            using var rpc = HostServer.CreateConnection(host.StandardInput.BaseStream, host.StandardOutput.BaseStream);
            var loaded = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
            rpc.AddLocalRpcMethod("workspace/projectInitializationComplete", new Action(() => loaded.TrySetResult()));
            rpc.StartListening();

            await rpc.InvokeWithParameterObjectAsync<JsonElement>("initialize", new { clientName = "legacy-webforms-test", clientVersion = "0", solutionPath = project }, Ct);
            await loaded.Task.WaitAsync(TimeSpan.FromMinutes(5), Ct);

            // The generated partial (not the checked-in .designer.cs) declares the probe fields.
            var generated = Directory.GetFiles(Path.Combine(cacheDir, "designtime", "generated"), "Login.aspx.g.cs", SearchOption.AllDirectories);
            var partial = Assert.Single(generated);
            var partialText = await File.ReadAllTextAsync(partial, Ct);
            Assert.Contains($"global::System.Web.UI.WebControls.Button {ProbeButton};", partialText, StringComparison.Ordinal);
            Assert.Contains($"global::System.Web.UI.WebControls.Label {ProbeLabel};", partialText, StringComparison.Ordinal);

            var uri = new Uri(codeBehind).AbsoluteUri;
            var original = await File.ReadAllTextAsync(codeBehind, Ct);
            const string anchor = "RegisterHyperLink.NavigateUrl = \"Register\";";
            var at = original.IndexOf(anchor, StringComparison.Ordinal);
            Assert.True(at > 0, "probe anchor not found in Login.aspx.cs");
            var insertAt = at + anchor.Length;
            var text = original[..insertAt] + $"\n            {ProbeButton}.Text = \"x\";\n            {ProbeLabel}.Text = \"y\";\n" + original[insertAt..];

            await rpc.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri, languageId = "csharp", version = 1, text } });
            // Full semantics for the document version (see the brief 0002 report on frozen-partial completion).
            var diagnostics = await rpc.InvokeWithParameterObjectAsync<JsonElement>("textDocument/diagnostic", new { textDocument = new { uri } }, Ct);
            var probeErrors = diagnostics.GetProperty("items").EnumerateArray()
                .Where(d => d.GetProperty("severity").GetInt32() == 1)
                .Select(d => d.GetProperty("message").GetString() ?? string.Empty)
                .Where(m => m.Contains("NielloProbe", StringComparison.Ordinal))
                .ToList();
            Assert.Empty(probeErrors);

            var buttonMembers = await CompleteAfterAsync(rpc, uri, text, $"{ProbeButton}.");
            Assert.Contains("Text", buttonMembers);
            Assert.Contains("OnClientClick", buttonMembers);
            Assert.Contains("CommandName", buttonMembers);

            var labelMembers = await CompleteAfterAsync(rpc, uri, text, $"{ProbeLabel}.");
            Assert.Contains("Text", labelMembers);
            Assert.Contains("AssociatedControlID", labelMembers);

            await rpc.InvokeWithCancellationAsync<JsonElement>("shutdown", [], Ct);
            await rpc.NotifyAsync("exit");
            await host.WaitForExitAsync(Ct).WaitAsync(TimeSpan.FromSeconds(30), Ct);
            Assert.Equal(0, host.ExitCode);
        }
        finally
        {
            if (!host.HasExited)
            {
                host.Kill(entireProcessTree: true);
            }

            var log = await stderr;
            TestContext.Current.TestOutputHelper?.WriteLine(log.Length > 6000 ? log[^6000..] : log);
        }
    }

    private static async Task<List<string?>> CompleteAfterAsync(StreamJsonRpc.JsonRpc rpc, string uri, string text, string probe)
    {
        var offset = text.IndexOf(probe, StringComparison.Ordinal) + probe.Length;
        var before = text[..offset];
        var line = before.Count(c => c == '\n');
        var character = offset - (before.LastIndexOf('\n') + 1);
        var completion = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/completion",
            new
            {
                textDocument = new { uri },
                position = new { line, character },
                context = new { triggerKind = 1 },
            },
            Ct);
        Assert.Equal(JsonValueKind.Object, completion.ValueKind);
        return completion.GetProperty("items").EnumerateArray().Select(i => i.GetProperty("label").GetString()).ToList();
    }

    private static string? FindCorpusProject()
    {
        const string relative = "aspnet-samples/samples/aspnet/Identity/ChangePK/PrimaryKeysConfigTest";
        var fromEnv = Environment.GetEnvironmentVariable("NIELLO_LEGACY_CORPUS");
        if (!string.IsNullOrEmpty(fromEnv))
        {
            var p = Path.Combine(fromEnv, relative);
            return Directory.Exists(p) ? p : null;
        }

        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "corpus", "legacy", ".checkout", relative);
            if (Directory.Exists(candidate))
            {
                return candidate;
            }
        }

        return null;
    }
}
