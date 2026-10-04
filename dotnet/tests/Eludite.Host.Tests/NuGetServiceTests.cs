using System.Diagnostics;
using System.Text.Json;
using Eludite.Host.NuGet;
using Eludite.Host.Rpc;
using Nerdbank.Streams;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0048: <c>eludite/nuget/*</c> over the wire, against the NuGet corpus copied to a temporary folder with its local
/// feed (<see cref="NuGetCorpus"/>) and, for the http cases, a fake V3 feed on loopback (<see cref="FakeV3Feed"/>):
/// search with the session cache and its budget, installed before and after a restore (transitive packages, the source),
/// install with the restore and the generation, uninstall, updates and update, consolidate across two projects, the
/// edit preserving every other byte, Central Package Management, a lock file, a failing source, vulnerability and
/// deprecation data from the registration resource, the credential round trip with the shell and a fake provider, the
/// refusal of a non-interactive call, cancellation mid-search, the generation rule, the sources' changes, and the
/// Dependencies node of the solution tree.
/// </summary>
public sealed class NuGetServiceTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private static string Read(string path) => File.ReadAllText(path);

    [Fact]
    public async Task Search_FindsTheLocalFeedsPackages_PrereleaseOnRequest_AndCachesPerSource()
    {
        var root = NuGetCorpus.Create();
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            var first = await host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Corpus" });
            var greeter = first.GetProperty("results").EnumerateArray().Single(r => r.GetProperty("id").GetString() == NuGetCorpus.Greeter);
            Assert.Equal("1.1.0", greeter.GetProperty("version").GetString());
            Assert.Equal(["1.1.0", "1.0.0"], greeter.GetProperty("versions").EnumerateArray().Select(v => v.GetString()));
            Assert.Equal("corpus", greeter.GetProperty("source").GetString());
            Assert.Equal("Greets people, for Eludite's NuGet tests.", greeter.GetProperty("description").GetString());
            var source = Assert.Single(first.GetProperty("sources").EnumerateArray());
            Assert.Equal(2, source.GetProperty("count").GetInt32());
            Assert.False(source.TryGetProperty("cached", out _));

            var pre = await host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Greeter", prerelease = true });
            var beta = Assert.Single(pre.GetProperty("results").EnumerateArray());
            Assert.Equal("2.0.0-beta.1", beta.GetProperty("version").GetString());
            Assert.Equal(["2.0.0-beta.1", "1.1.0", "1.0.0"], beta.GetProperty("versions").EnumerateArray().Select(v => v.GetString()));

            // The same search again: the session's cache answers, under the budget.
            var requests = host.Target.NuGet.Feeds.Requests;
            var watch = Stopwatch.StartNew();
            var again = await host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Corpus" });
            watch.Stop();
            Assert.True(again.GetProperty("sources")[0].GetProperty("cached").GetBoolean());
            Assert.Equal(requests, host.Target.NuGet.Feeds.Requests);
            // A cold search of the local feed (a query not asked before) under 200 ms.
            var cold = Stopwatch.StartNew();
            await host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Logging" });
            cold.Stop();
            TestContext.Current.SendDiagnosticMessage($"search on the local feed: cold {cold.ElapsedMilliseconds} ms, cached {watch.ElapsedMilliseconds} ms");
            Assert.True(cold.ElapsedMilliseconds < 200 * Slack(), $"a local search took {cold.ElapsedMilliseconds} ms");
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Installed_ReadsTheProjectFileBeforeRestore_AndTheAssetsFileAfter()
    {
        var root = NuGetCorpus.Create();
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            var app = NuGetCorpus.Project(root, "App");
            var before = await host.CallAsync("eludite/nuget/installed", new { generation = host.Generation, projects = new[] { app } });
            var project = Assert.Single(before.GetProperty("projects").EnumerateArray());
            Assert.False(project.GetProperty("restored").GetBoolean());
            var listed = Assert.Single(project.GetProperty("packages").EnumerateArray());
            Assert.Equal("1.0.0", listed.GetProperty("requested").GetString());
            Assert.False(listed.TryGetProperty("version", out _));
            Assert.Equal(0, host.Target.NuGet.Feeds.Requests);

            var restore = await host.CallAsync("eludite/nuget/restore", new { generation = host.Generation, operation = 7 });
            Assert.Equal("succeeded", restore.GetProperty("result").GetString());
            Assert.False(restore.GetProperty("lockedMode").GetBoolean());
            Assert.Contains(host.Output(7), l => l.Contains("Restored", StringComparison.Ordinal));

            var after = await host.CallAsync("eludite/nuget/installed", new { generation = host.Generation, includeTransitive = true });
            var projects = after.GetProperty("projects").EnumerateArray().ToList();
            Assert.Equal(["App", "Lib", "Shared"], projects.Select(p => p.GetProperty("name").GetString()));
            var packages = projects[0].GetProperty("packages").EnumerateArray().ToList();
            Assert.Equal([NuGetCorpus.Greeter, NuGetCorpus.Logging], packages.Select(p => p.GetProperty("id").GetString()));
            Assert.Equal("1.0.0", packages[0].GetProperty("version").GetString());
            Assert.Equal("1.0.0", packages[0].GetProperty("requested").GetString());
            Assert.False(packages[0].GetProperty("transitive").GetBoolean());
            Assert.True(packages[1].GetProperty("transitive").GetBoolean());
            Assert.Equal(Path.Combine(root, "feed"), packages[0].GetProperty("source").GetString());
            Assert.Equal(NuGetCorpus.Logging, packages[0].GetProperty("dependencies")[0].GetProperty("id").GetString());
            Assert.Equal("1.1.0", projects[1].GetProperty("packages")[0].GetProperty("version").GetString());
            Assert.Equal("none", projects[2].GetProperty("format").GetString());
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Install_EditsTheProjectRestoresAndAdvancesTheGeneration_ThenUninstallRemovesIt()
    {
        var root = NuGetCorpus.Create();
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            var shared = NuGetCorpus.Project(root, "Shared");
            var generation = host.Generation;
            var watch = Stopwatch.StartNew();
            var install = await host.CallAsync("eludite/nuget/change", new
            {
                generation,
                operation = 3,
                action = "install",
                packages = new[] { new { id = "eludite.corpus.logging" } },
                projects = new[] { shared },
            });
            watch.Stop();
            TestContext.Current.SendDiagnosticMessage($"install plus restore: {watch.ElapsedMilliseconds} ms");
            Assert.Equal(generation + 1, install.GetProperty("generation").GetInt64());
            Assert.Equal(generation + 1, host.Generation);
            Assert.Equal("1.0.0", install.GetProperty("packages")[0].GetProperty("version").GetString());
            Assert.Equal(NuGetCorpus.Logging, install.GetProperty("packages")[0].GetProperty("id").GetString());
            var edited = Assert.Single(install.GetProperty("edited").EnumerateArray());
            Assert.Equal(shared, edited.GetProperty("path").GetString());
            Assert.Equal("project", edited.GetProperty("kind").GetString());
            Assert.Equal("succeeded", install.GetProperty("restore").GetProperty("result").GetString());
            Assert.Contains("<PackageReference Include=\"Eludite.Corpus.Logging\" Version=\"1.0.0\" />", Read(shared));
            Assert.True(File.Exists(InstalledReader.AssetsPath(shared)));
            var output = host.Output(3);
            Assert.Contains(output, l => l.StartsWith("Installing NuGet package Eludite.Corpus.Logging 1.0.0 in Shared", StringComparison.Ordinal));
            Assert.Contains(output, l => l.StartsWith("Successfully installed 'Eludite.Corpus.Logging 1.0.0' to Shared", StringComparison.Ordinal));
            Assert.Equal("========== Finished ==========", output[^1]);
            Assert.True(watch.ElapsedMilliseconds < 5000 * Slack(), $"install plus restore took {watch.ElapsedMilliseconds} ms");

            // The old generation is stale now.
            var (code, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/installed", new { generation }));
            Assert.Equal(HostErrors.ContentModified, code);

            var uninstall = await host.CallAsync("eludite/nuget/change", new
            {
                generation = host.Generation,
                action = "uninstall",
                packages = new[] { new { id = NuGetCorpus.Logging } },
            });
            Assert.Equal([shared], uninstall.GetProperty("projects").EnumerateArray().Select(p => p.GetString()));
            Assert.DoesNotContain("PackageReference", Read(shared));
            Assert.DoesNotContain("<ItemGroup>", Read(shared));
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Updates_ListsNewerVersions_AndUpdateAndConsolidateSettleTheProjects()
    {
        var root = NuGetCorpus.Create();
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            var (app, lib) = (NuGetCorpus.Project(root, "App"), NuGetCorpus.Project(root, "Lib"));
            await host.CallAsync("eludite/nuget/restore", new { generation = host.Generation });
            var updates = await host.CallAsync("eludite/nuget/updates", new { generation = host.Generation });
            var row = Assert.Single(updates.GetProperty("updates").EnumerateArray());
            Assert.Equal(app, row.GetProperty("project").GetString());
            Assert.Equal("1.0.0", row.GetProperty("installed").GetString());
            Assert.Equal("1.1.0", row.GetProperty("latest").GetString());
            var pre = await host.CallAsync("eludite/nuget/updates", new { generation = host.Generation, prerelease = true });
            Assert.Equal(2, pre.GetProperty("updates").GetArrayLength());
            Assert.All(pre.GetProperty("updates").EnumerateArray(), r => Assert.Equal("2.0.0-beta.1", r.GetProperty("latest").GetString()));

            // Consolidate: App 1.0.0 and Lib 1.1.0 settle on the highest.
            var consolidate = await host.CallAsync("eludite/nuget/change", new
            {
                generation = host.Generation,
                action = "consolidate",
                packages = new[] { new { id = NuGetCorpus.Greeter } },
            });
            Assert.Equal("1.1.0", consolidate.GetProperty("packages")[0].GetProperty("version").GetString());
            Assert.Equal([app], consolidate.GetProperty("projects").EnumerateArray().Select(p => p.GetString()));
            Assert.Contains("Include=\"Eludite.Corpus.Greeter\" Version=\"1.1.0\"", Read(app));

            // Update to a named prerelease version in every project that references it.
            var update = await host.CallAsync("eludite/nuget/change", new
            {
                generation = host.Generation,
                action = "update",
                packages = new[] { new { id = NuGetCorpus.Greeter, version = "2.0.0-beta.1" } },
            });
            Assert.Equal([app, lib], update.GetProperty("projects").EnumerateArray().Select(p => p.GetString()).Order());
            Assert.Contains("Version=\"2.0.0-beta.1\"", Read(lib));
            var none = await host.CallAsync("eludite/nuget/updates", new { generation = host.Generation, prerelease = true });
            Assert.Equal(0, none.GetProperty("updates").GetArrayLength());

            // A version no source has fails the whole change and writes nothing.
            var text = Read(app);
            var (code, data) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/change", new
            {
                generation = host.Generation,
                action = "update",
                packages = new[] { new { id = NuGetCorpus.Greeter, version = "9.9.9" } },
            }));
            Assert.Equal(NuGetService.NuGetFailed, code);
            Assert.Equal("notFound", data!.Value.GetProperty("reason").GetString());
            Assert.Equal(text, Read(app));
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Edit_KeepsEveryOtherByte_CommentsIndentationAndOrder()
    {
        var root = NuGetCorpus.Create();
        try
        {
            var lib = NuGetCorpus.Project(root, "Lib");
            const string Original = "<Project Sdk=\"Microsoft.NET.Sdk\">\r\n" +
                "  <!-- a comment the edit keeps -->\r\n" +
                "\t<PropertyGroup>\r\n" +
                "\t\t<TargetFramework>net10.0</TargetFramework>\r\n" +
                "\t</PropertyGroup>\r\n" +
                "      <ItemGroup Label=\"Packages\">\r\n" +
                "            <PackageReference   Include=\"Eludite.Corpus.Greeter\"   Version=\"1.0.0\" /> <!-- pinned -->\r\n" +
                "      </ItemGroup>\r\n" +
                "  <ItemGroup>\r\n" +
                "    <None Include=\"readme.txt\" />\r\n" +
                "  </ItemGroup>\r\n" +
                "</Project>\r\n";
            File.WriteAllText(lib, Original);
            await using var host = await NuGetHost.OpenAsync(root);
            await host.CallAsync("eludite/nuget/change", new
            {
                generation = host.Generation,
                action = "update",
                packages = new[] { new { id = NuGetCorpus.Greeter, version = "1.1.0" } },
                projects = new[] { lib },
                restore = false,
            });
            // Every other line byte for byte (line endings, tabs, comments, the other ItemGroup); the changed element keeps
            // its place, indentation and trailing comment, its attributes written as MSBuild writes them.
            var before = Original.Split("\r\n");
            var after = Read(lib).Split("\r\n");
            Assert.Equal(before.Length, after.Length);
            for (var i = 0; i < before.Length; i++)
            {
                if (i == 6)
                {
                    Assert.Equal("            <PackageReference Include=\"Eludite.Corpus.Greeter\" Version=\"1.1.0\" /> <!-- pinned -->", after[i]);
                }
                else
                {
                    Assert.Equal(before[i], after[i]);
                }
            }
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task CentralPackageManagement_EditsDirectoryPackagesProps_AndVersionlessReferences()
    {
        var root = NuGetCorpus.Create();
        try
        {
            File.WriteAllText(Path.Combine(root, "Directory.Packages.props"),
                "<Project>\n  <PropertyGroup>\n    <ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>\n  </PropertyGroup>\n" +
                "  <ItemGroup>\n    <PackageVersion Include=\"Eludite.Corpus.Greeter\" Version=\"1.0.0\" />\n  </ItemGroup>\n</Project>\n");
            var (app, lib) = (NuGetCorpus.Project(root, "App"), NuGetCorpus.Project(root, "Lib"));
            File.WriteAllText(app, Read(app).Replace(" Version=\"1.0.0\"", string.Empty, StringComparison.Ordinal));
            File.WriteAllText(lib, Read(lib).Replace(" Version=\"1.1.0\"", string.Empty, StringComparison.Ordinal));
            var appText = Read(app);
            await using var host = await NuGetHost.OpenAsync(root);
            var installed = await host.CallAsync("eludite/nuget/installed", new { generation = host.Generation, projects = new[] { app } });
            var project = installed.GetProperty("projects")[0];
            Assert.True(project.GetProperty("centralPackageManagement").GetBoolean());
            Assert.Equal("1.0.0", project.GetProperty("packages")[0].GetProperty("requested").GetString());

            var update = await host.CallAsync("eludite/nuget/change", new
            {
                generation = host.Generation,
                action = "update",
                packages = new[] { new { id = NuGetCorpus.Greeter, version = "1.1.0" } },
            });
            var edited = Assert.Single(update.GetProperty("edited").EnumerateArray());
            Assert.Equal("centralPackageVersions", edited.GetProperty("kind").GetString());
            Assert.Contains("<PackageVersion Include=\"Eludite.Corpus.Greeter\" Version=\"1.1.0\" />", Read(Path.Combine(root, "Directory.Packages.props")));
            Assert.Equal(appText, Read(app));
            Assert.Equal("succeeded", update.GetProperty("restore").GetProperty("result").GetString());

            var install = await host.CallAsync("eludite/nuget/change", new
            {
                generation = host.Generation,
                action = "install",
                packages = new[] { new { id = NuGetCorpus.Logging, version = "1.0.0" } },
                projects = new[] { lib },
            });
            Assert.Equal(2, install.GetProperty("edited").GetArrayLength());
            Assert.Contains("<PackageReference Include=\"Eludite.Corpus.Logging\" />", Read(lib));
            Assert.Contains("<PackageVersion Include=\"Eludite.Corpus.Logging\" Version=\"1.0.0\" />", Read(Path.Combine(root, "Directory.Packages.props")));
            var after = await host.CallAsync("eludite/nuget/installed", new { generation = host.Generation, projects = new[] { lib } });
            Assert.Contains(after.GetProperty("projects")[0].GetProperty("packages").EnumerateArray(), p => p.GetProperty("id").GetString() == NuGetCorpus.Logging && p.GetProperty("version").GetString() == "1.0.0");
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task LockFile_IsRespectedOnRestore_AndUpdatedByAChange()
    {
        var root = NuGetCorpus.Create();
        try
        {
            var lib = NuGetCorpus.Project(root, "Lib");
            File.WriteAllText(lib, Read(lib).Replace("<TargetFramework>net10.0</TargetFramework>", "<TargetFramework>net10.0</TargetFramework>\n    <RestorePackagesWithLockFile>true</RestorePackagesWithLockFile>", StringComparison.Ordinal));
            await using var host = await NuGetHost.OpenAsync(root);
            var first = await host.CallAsync("eludite/nuget/restore", new { generation = host.Generation, projects = new[] { lib } });
            Assert.Equal("succeeded", first.GetProperty("result").GetString());
            var lockFile = Path.Combine(root, "Lib", "packages.lock.json");
            Assert.Equal([lockFile], first.GetProperty("lockFiles").EnumerateArray().Select(l => l.GetString()));
            Assert.Contains("\"resolved\": \"1.1.0\"", Read(lockFile));

            var locked = await host.CallAsync("eludite/nuget/restore", new { generation = host.Generation, projects = new[] { lib } });
            Assert.True(locked.GetProperty("lockedMode").GetBoolean());
            Assert.Contains("--locked-mode", locked.GetProperty("commandLine").GetString());

            var update = await host.CallAsync("eludite/nuget/change", new
            {
                generation = host.Generation,
                action = "update",
                packages = new[] { new { id = NuGetCorpus.Greeter, version = "2.0.0-beta.1" } },
                projects = new[] { lib },
            });
            var restore = update.GetProperty("restore");
            Assert.Equal("succeeded", restore.GetProperty("result").GetString());
            Assert.False(restore.GetProperty("lockedMode").GetBoolean());
            Assert.Contains("--force-evaluate", restore.GetProperty("commandLine").GetString());
            Assert.Contains("\"resolved\": \"2.0.0-beta.1\"", Read(lockFile));

            // ignore: neither switch.
            var ignored = await host.CallAsync("eludite/nuget/restore", new { generation = host.Generation, projects = new[] { lib }, lockFiles = "ignore" });
            Assert.False(ignored.GetProperty("lockedMode").GetBoolean());
            Assert.DoesNotContain("--locked-mode", ignored.GetProperty("commandLine").GetString());
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task AFailingSource_AnswersItsRow_WhileTheOthersResultsShow_AndAFailedRestoreKeepsTheEdit()
    {
        var root = NuGetCorpus.Create();
        try
        {
            File.WriteAllText(Path.Combine(root, "NuGet.config"), Read(Path.Combine(root, "NuGet.config"))
                .Replace("<add key=\"corpus\" value=\"feed\" />", "<add key=\"corpus\" value=\"feed\" />\n    <add key=\"down\" value=\"http://127.0.0.1:9/v3/index.json\" allowInsecureConnections=\"true\" />", StringComparison.Ordinal));
            await using var host = await NuGetHost.OpenAsync(root);
            var search = await host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Greeter" });
            Assert.Single(search.GetProperty("results").EnumerateArray());
            var rows = search.GetProperty("sources").EnumerateArray().ToList();
            Assert.Equal(["corpus", "down"], rows.Select(r => r.GetProperty("name").GetString()));
            Assert.False(rows[0].TryGetProperty("error", out _));
            Assert.False(string.IsNullOrEmpty(rows[1].GetProperty("error").GetString()));

            // Only the dead source: the call fails as a whole.
            var (code, data) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Greeter", source = "down" }));
            Assert.Equal(NuGetService.NuGetFailed, code);
            Assert.Equal("sourceFailed", data!.Value.GetProperty("reason").GetString());

            // A restore that fails (Lib asks for a version the feed does not have) leaves the edit and reports NU1102.
            var lib = NuGetCorpus.Project(root, "Lib");
            File.WriteAllText(Path.Combine(root, "NuGet.config"), Read(Path.Combine(root, "NuGet.config")).Replace("\n    <add key=\"down\" value=\"http://127.0.0.1:9/v3/index.json\" allowInsecureConnections=\"true\" />", string.Empty, StringComparison.Ordinal));
            File.WriteAllText(lib, Read(lib).Replace("Version=\"1.1.0\"", "Version=\"7.0.0\"", StringComparison.Ordinal));
            var restore = await host.CallAsync("eludite/nuget/restore", new { generation = host.Generation });
            Assert.Equal("failed", restore.GetProperty("result").GetString());
            var error = Assert.Single(restore.GetProperty("diagnostics").EnumerateArray(), d => d.GetProperty("severity").GetString() == "error");
            Assert.Equal("NU1102", error.GetProperty("code").GetString());
            Assert.Equal(lib, error.GetProperty("file").GetString());
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Vulnerabilities_AndDeprecation_ComeFromTheRegistrationResource()
    {
        var root = NuGetCorpus.Create();
        await using var feed = new FakeV3Feed(new() { [NuGetCorpus.Greeter] = ["1.0.0", "1.1.0"], [NuGetCorpus.Logging] = ["1.0.0"] });
        feed.Vulnerable["eludite.corpus.greeter/1.0.0"] = (2, "https://github.com/advisories/GHSA-test-0048");
        feed.Deprecated["eludite.corpus.greeter/1.0.0"] = "Use 1.1.0";
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            await host.CallAsync("eludite/nuget/restore", new { generation = host.Generation });
            AddSource(root, "fake", feed.Url);
            var installed = await host.CallAsync("eludite/nuget/installed", new { generation = host.Generation, operation = 11, metadata = true, projects = new[] { NuGetCorpus.Project(root, "App") } });
            var greeter = installed.GetProperty("projects")[0].GetProperty("packages")[0];
            var vulnerability = Assert.Single(greeter.GetProperty("vulnerabilities").EnumerateArray());
            Assert.Equal("high", vulnerability.GetProperty("severity").GetString());
            Assert.Equal("https://github.com/advisories/GHSA-test-0048", vulnerability.GetProperty("advisoryUrl").GetString());
            Assert.Equal(["legacy"], greeter.GetProperty("deprecation").GetProperty("reasons").EnumerateArray().Select(r => r.GetString()));
            Assert.Equal("Use 1.1.0", greeter.GetProperty("deprecation").GetProperty("message").GetString());
            var metadata = Assert.Single(host.Updates(11), u => u.GetProperty("kind").GetString() == "metadata");
            Assert.Equal(NuGetCorpus.Greeter, metadata.GetProperty("packages")[0].GetProperty("id").GetString());
            // Updates carry the installed version's vulnerability as a badge.
            var updates = await host.CallAsync("eludite/nuget/updates", new { generation = host.Generation });
            Assert.Single(updates.GetProperty("updates").EnumerateArray());
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Credentials_AreAskedOfTheShellForTheInteractiveCall_KeptForTheSession_AndRefusedOtherwise()
    {
        var root = NuGetCorpus.Create();
        await using var feed = new FakeV3Feed(new() { ["Private.Package"] = ["3.0.0"] }, ("alice", "s3cret"));
        try
        {
            var asked = new List<JsonElement>();
            await using var host = await NuGetHost.OpenAsync(root, p =>
            {
                lock (asked)
                {
                    asked.Add(p.Clone());
                }

                return new { username = "alice", password = asked.Count == 1 ? "wrong" : "s3cret" };
            });
            AddSource(root, "private", feed.Url, clear: true);

            // An agent's call (not interactive): refused with credentials_required and the host, nobody asked.
            var (code, data) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Private" }));
            Assert.Equal(NuGetService.NuGetFailed, code);
            Assert.Equal("credentialsRequired", data!.Value.GetProperty("reason").GetString());
            Assert.Equal(feed.Authority, data.Value.GetProperty("host").GetString());
            Assert.Empty(asked);

            // The person's call: the shell is asked; a refused answer is asked again as a retry.
            var found = await host.CallAsync("eludite/nuget/search", new { generation = host.Generation, operation = 5, query = "Private", interactive = true });
            Assert.Equal("Private.Package", found.GetProperty("results")[0].GetProperty("id").GetString());
            Assert.Equal(2, asked.Count);
            Assert.Equal("private", asked[0].GetProperty("source").GetString());
            Assert.Equal(feed.Authority, asked[0].GetProperty("host").GetString());
            Assert.False(asked[0].GetProperty("isRetry").GetBoolean());
            Assert.True(asked[1].GetProperty("isRetry").GetBoolean());
            Assert.Equal(5, asked[0].GetProperty("operation").GetInt64());

            // Kept for the session: an agent's call now gets through without asking.
            var versions = await host.CallAsync("eludite/nuget/updates", new { generation = host.Generation });
            Assert.Equal(2, asked.Count);
            Assert.True(versions.GetProperty("sources")[0].TryGetProperty("count", out _));
            var again = await host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Package" });
            Assert.Single(again.GetProperty("results").EnumerateArray());
            Assert.Equal(2, asked.Count);
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task ACredentialProvider_AnswersBeforeTheShell()
    {
        var root = NuGetCorpus.Create();
        await using var feed = new FakeV3Feed(new() { ["Provided.Package"] = ["1.0.0"] }, ("bob", "token"));
        try
        {
            FakeCredentialProvider.Answer(feed.Authority, "bob", "token");
            var asked = 0;
            await using var host = await NuGetHost.OpenAsync(root, _ =>
            {
                Interlocked.Increment(ref asked);
                return null;
            });
            AddSource(root, "provided", feed.Url, clear: true);
            var found = await host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "Provided", interactive = true });
            Assert.Single(found.GetProperty("results").EnumerateArray());
            Assert.Equal(0, asked);
            Assert.True(FakeCredentialProvider.Calls(feed.Authority) >= 1);
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task ASearchIsCanceledMidway()
    {
        var root = NuGetCorpus.Create();
        await using var feed = new FakeV3Feed(new() { ["Slow.Package"] = ["1.0.0"] }) { QueryDelay = TimeSpan.FromSeconds(20) };
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            AddSource(root, "slow", feed.Url, clear: true);
            using var cancel = new CancellationTokenSource();
            var call = host.Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/nuget/search", new { generation = host.Generation, query = "Slow" }, cancel.Token);
            var deadline = Stopwatch.StartNew();
            while (feed.Requests < 2 && deadline.Elapsed < TimeSpan.FromSeconds(30))
            {
                await Task.Delay(10, Ct);
            }

            var watch = Stopwatch.StartNew();
            await cancel.CancelAsync();
            await Assert.ThrowsAnyAsync<OperationCanceledException>(() => call);
            Assert.True(watch.Elapsed < TimeSpan.FromSeconds(5), $"the cancel took {watch.ElapsedMilliseconds} ms");
            // The host is still serving.
            var sources = await host.CallAsync("eludite/nuget/sources", new { generation = host.Generation });
            Assert.Single(sources.GetProperty("sources").EnumerateArray());
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task TheGenerationRule_AndBadParams()
    {
        var root = NuGetCorpus.Create();
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            var (missing, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/search", new { query = "x" }));
            Assert.Equal(HostErrors.InvalidParams, missing);
            var (stale, data) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/search", new { generation = host.Generation + 5 }));
            Assert.Equal(HostErrors.ContentModified, stale);
            Assert.Equal(host.Generation, data!.Value.GetProperty("currentGeneration").GetInt64());
            var (noProjects, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/change", new { generation = host.Generation, action = "install", packages = new[] { new { id = NuGetCorpus.Logging } } }));
            Assert.Equal(HostErrors.InvalidParams, noProjects);
            var (foreign, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/installed", new { generation = host.Generation, projects = new[] { "/nowhere/X.csproj" } }));
            Assert.Equal(HostErrors.InvalidParams, foreign);
            var (empty, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/change", new { generation = host.Generation, action = "update", packages = Array.Empty<object>() }));
            Assert.Equal(HostErrors.InvalidParams, empty);
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Sources_ListTheChain_AndChangesGoToTheUserFile()
    {
        var root = NuGetCorpus.Create();
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            var list = await host.CallAsync("eludite/nuget/sources", new { generation = host.Generation });
            var corpus = Assert.Single(list.GetProperty("sources").EnumerateArray());
            Assert.Equal("solution", corpus.GetProperty("scope").GetString());
            Assert.True(corpus.GetProperty("local").GetBoolean());
            Assert.Equal(Path.Combine(root, "NuGet.config"), corpus.GetProperty("configFile").GetString());
            var user = host.Target.NuGet.Chain.UserConfig;
            Assert.Equal(user, list.GetProperty("userConfig").GetString());
            Assert.False(File.Exists(user));

            // The corpus's <clear/> hides a user source: it is written, but NuGet does not list it for this solution.
            var added = await host.CallAsync("eludite/nuget/sources", new { generation = host.Generation, action = "add", name = "mine", url = "https://example.invalid/v3/index.json" });
            Assert.True(added.GetProperty("changed").GetBoolean());
            Assert.Contains("<add key=\"mine\" value=\"https://example.invalid/v3/index.json\" />", Read(user));
            Assert.Single(added.GetProperty("sources").EnumerateArray());

            var disabled = await host.CallAsync("eludite/nuget/sources", new { generation = host.Generation, action = "disable", name = "corpus" });
            Assert.False(disabled.GetProperty("sources")[0].GetProperty("enabled").GetBoolean());
            Assert.Contains("<add key=\"corpus\" value=\"true\" />", Read(user));
            var (none, noneData) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/search", new { generation = host.Generation, query = "x" }));
            Assert.Equal(NuGetService.NuGetFailed, none);
            Assert.Equal("noSources", noneData!.Value.GetProperty("reason").GetString());
            var enabled = await host.CallAsync("eludite/nuget/sources", new { generation = host.Generation, action = "enable", name = "corpus" });
            Assert.True(enabled.GetProperty("sources")[0].GetProperty("enabled").GetBoolean());

            var removed = await host.CallAsync("eludite/nuget/sources", new { generation = host.Generation, action = "remove", name = "mine" });
            Assert.DoesNotContain("mine", Read(user));
            Assert.True(removed.GetProperty("changed").GetBoolean());
            var (cannot, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/nuget/sources", new { generation = host.Generation, action = "remove", name = "corpus" }));
            Assert.Equal(HostErrors.InvalidParams, cannot);
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task TheSolutionTree_HasTheDependenciesNode()
    {
        var root = NuGetCorpus.Create();
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            await host.CallAsync("eludite/nuget/restore", new { generation = host.Generation });
            var tree = await host.CallAsync("eludite/solution/tree", null);
            var app = tree.GetProperty("projects").EnumerateArray().Single(p => p.GetProperty("name").GetString() == "App");
            var deps = app.GetProperty("dependencies");
            Assert.True(deps.GetProperty("restored").GetBoolean());
            var greeter = Assert.Single(deps.GetProperty("packages").EnumerateArray());
            Assert.Equal(NuGetCorpus.Greeter, greeter.GetProperty("id").GetString());
            Assert.Equal("1.0.0", greeter.GetProperty("version").GetString());
            Assert.Equal(NuGetCorpus.Logging, greeter.GetProperty("transitive")[0].GetProperty("id").GetString());
            var reference = Assert.Single(deps.GetProperty("projects").EnumerateArray());
            Assert.Equal("Shared", reference.GetProperty("name").GetString());
            Assert.Equal(NuGetCorpus.Project(root, "Shared"), reference.GetProperty("path").GetString());
            Assert.Contains(deps.GetProperty("frameworks").EnumerateArray(), f => f.GetProperty("name").GetString() == "Microsoft.NETCore.App");
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Installed_OfTwentyProjects_FromTheirAssetsFiles_IsUnderBudget()
    {
        var root = NuGetCorpus.Create();
        try
        {
            await using (var restorer = await NuGetHost.OpenAsync(root))
            {
                await restorer.CallAsync("eludite/nuget/restore", new { generation = restorer.Generation, projects = new[] { NuGetCorpus.Project(root, "App") } });
            }

            // Twenty restored projects: copies of App with its assets file.
            var slnx = new System.Text.StringBuilder("<Solution>\n");
            for (var i = 0; i < 20; i++)
            {
                var dir = Path.Combine(root, $"P{i}");
                Directory.CreateDirectory(Path.Combine(dir, "obj"));
                File.Copy(NuGetCorpus.Project(root, "App"), Path.Combine(dir, $"P{i}.csproj"));
                File.Copy(InstalledReader.AssetsPath(NuGetCorpus.Project(root, "App")), Path.Combine(dir, "obj", "project.assets.json"));
                slnx.Append(System.Globalization.CultureInfo.InvariantCulture, $"  <Project Path=\"P{i}/P{i}.csproj\" />\n");
            }

            await File.WriteAllTextAsync(Path.Combine(root, "Many.slnx"), slnx.Append("</Solution>\n").ToString(), Ct);
            await using var host = await NuGetHost.OpenAsync(root, solution: "Many.slnx");
            await host.CallAsync("eludite/nuget/installed", new { generation = host.Generation });
            var times = new List<long>();
            for (var i = 0; i < 5; i++)
            {
                var watch = Stopwatch.StartNew();
                var result = await host.CallAsync("eludite/nuget/installed", new { generation = host.Generation, includeTransitive = true });
                times.Add(watch.ElapsedMilliseconds);
                Assert.Equal(20, result.GetProperty("projects").GetArrayLength());
            }

            times.Sort();
            TestContext.Current.SendDiagnosticMessage($"installed, 20 projects: median {times[2]} ms ({string.Join(", ", times)})");
            Assert.True(times[2] < 300 * Slack(), $"installed took {times[2]} ms");
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    [Fact]
    public async Task Icons_AreFetchedOnceIntoTheCacheFolder()
    {
        var root = NuGetCorpus.Create();
        await using var feed = new FakeV3Feed(new() { ["X"] = ["1.0.0"] });
        try
        {
            await using var host = await NuGetHost.OpenAsync(root);
            var first = await host.CallAsync("eludite/nuget/icon", new { url = feed.Base + "/icon.png" });
            var path = first.GetProperty("path").GetString()!;
            Assert.StartsWith(host.Target.NuGet.IconFolder, path, StringComparison.Ordinal);
            Assert.Equal([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], File.ReadAllBytes(path));
            var again = await host.CallAsync("eludite/nuget/icon", new { url = feed.Base + "/icon.png" });
            Assert.Equal(path, again.GetProperty("path").GetString());
            Assert.Equal(1, feed.Icons);
            var missing = await host.CallAsync("eludite/nuget/icon", new { url = feed.Base + "/missing.png" });
            Assert.Equal(JsonValueKind.Null, missing.GetProperty("path").ValueKind);
            var local = Path.Combine(root, "feed", "local.png");
            File.WriteAllBytes(local, [1, 2, 3]);
            var file = await host.CallAsync("eludite/nuget/icon", new { url = new Uri(local).AbsoluteUri });
            Assert.Equal(local, file.GetProperty("path").GetString());
        }
        finally
        {
            TestDirectory.Delete(root);
        }
    }

    /// <summary>Budgets get room on an overloaded machine (other agents build beside these tests).</summary>
    private static int Slack() => Environment.ProcessorCount >= 4 && Environment.GetEnvironmentVariable("ELUDITE_STRICT_BUDGETS") == "1" ? 1 : 3;

    private static void AddSource(string root, string name, string url, bool clear = false)
    {
        var config = Path.Combine(root, "NuGet.config");
        var text = Read(config);
        var line = $"<add key=\"{name}\" value=\"{url}\" allowInsecureConnections=\"true\" />";
        text = clear
            ? text.Replace("<add key=\"corpus\" value=\"feed\" />", line, StringComparison.Ordinal)
            : text.Replace("<add key=\"corpus\" value=\"feed\" />", "<add key=\"corpus\" value=\"feed\" />\n    " + line, StringComparison.Ordinal);
        File.WriteAllText(config, text);
    }

    /// <summary>A host on a duplex stream with the NuGet service on the corpus's chain (no machine-wide files, the user's file in the temp folder).</summary>
    private sealed class NuGetHost : IAsyncDisposable
    {
        private readonly Task<int> _server;
        private readonly List<JsonElement> _updates = [];

        private NuGetHost(string root, Func<JsonElement, object?>? credentials)
        {
            var (clientStream, serverStream) = FullDuplexStream.CreatePair();
            var chain = new NuGetConfigChain(Path.Combine(root, "user", "NuGet.Config"), machineWide: false);
            Target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null,
                nuget: new NuGetService(() => Target!.LanguageServer.CurrentSolution(), () => Target!.LanguageServer.AdvanceGeneration(), TextWriter.Null, chain)
                {
                    IconFolder = Path.Combine(root, "icons"),
                });
            _server = HostServer.RunAsync(serverStream, serverStream, Target);
            Client = TestRpc.Create(clientStream);
            TestRpc.On(Client, "eludite/nuget/update", p =>
            {
                lock (_updates)
                {
                    _updates.Add(p.Clone());
                }
            });
            var answer = credentials ?? (_ => null);
            Client.AddLocalRpcMethod(answer.Method, answer.Target, new JsonRpcMethodAttribute("eludite/nuget/credentials") { UseSingleObjectParameterDeserialization = true });
            Client.StartListening();
        }

        public static async Task<NuGetHost> OpenAsync(string root, Func<JsonElement, object?>? credentials = null, string solution = "Corpus.slnx")
        {
            _ = NuGetCredentials.Instance;
            NuGetCredentials.Instance.ProvidersOverride = [FakeCredentialProvider.Instance];
            var host = new NuGetHost(root, credentials);
            await host.CallAsync("eludite/host/initialize", new { clientName = "test", clientVersion = "0" });
            await host.CallAsync("eludite/solution/open", new { path = Path.Combine(root, solution) });
            return host;
        }

        public HostRpcTarget Target { get; }

        public JsonRpc Client { get; }

        public long Generation => Target.LanguageServer.Generation;

        public Task<JsonElement> CallAsync(string method, object? parameters) =>
            Client.InvokeWithParameterObjectAsync<JsonElement>(method, parameters, Ct);

        public List<JsonElement> Updates(long operation)
        {
            lock (_updates)
            {
                return _updates.Where(u => u.GetProperty("operation").GetInt64() == operation).OrderBy(u => u.GetProperty("seq").GetInt64()).ToList();
            }
        }

        /// <summary>The Package Manager lines of an operation, in order.</summary>
        public List<string> Output(long operation) =>
            Updates(operation).Where(u => u.GetProperty("kind").GetString() == "output")
                .SelectMany(u => u.GetProperty("text").GetString()!.Split('\n', StringSplitOptions.RemoveEmptyEntries))
                .ToList();

        public async ValueTask DisposeAsync()
        {
            Client.Dispose();
            await _server.WaitAsync(TimeSpan.FromSeconds(10));
        }
    }
}

/// <summary>A NuGet credential provider (as a plugin would be) that answers for the hosts a test names.</summary>
internal sealed class FakeCredentialProvider : global::NuGet.Credentials.ICredentialProvider
{
    private static readonly System.Collections.Concurrent.ConcurrentDictionary<string, (string User, string Password)> Answers = new();
    private static readonly System.Collections.Concurrent.ConcurrentDictionary<string, int> Asked = new();

    public static FakeCredentialProvider Instance { get; } = new();

    public string Id => "eludite-test-provider";

    public static void Answer(string authority, string user, string password) => Answers[authority] = (user, password);

    public static int Calls(string authority) => Asked.GetValueOrDefault(authority);

    public Task<global::NuGet.Credentials.CredentialResponse> GetAsync(Uri uri, System.Net.IWebProxy proxy, global::NuGet.Configuration.CredentialRequestType type, string message, bool isRetry, bool nonInteractive, CancellationToken cancellationToken)
    {
        Asked.AddOrUpdate(uri.Authority, 1, (_, n) => n + 1);
        return Task.FromResult(Answers.TryGetValue(uri.Authority, out var a) && !isRetry
            ? new global::NuGet.Credentials.CredentialResponse(new System.Net.NetworkCredential(a.User, a.Password))
            : new global::NuGet.Credentials.CredentialResponse(global::NuGet.Credentials.CredentialStatus.ProviderNotApplicable));
    }
}
