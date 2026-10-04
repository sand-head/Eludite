namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0049's corpus (<c>corpus/projects</c>), copied to a temporary folder per test so edits never touch the
/// repository.
/// </summary>
internal sealed class ProjectCorpus : IDisposable
{
    public ProjectCorpus()
    {
        Root = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "eludite-projects-" + Guid.NewGuid().ToString("N")[..8]);
        Copy(Source(), Root);
    }

    public string Root { get; }

    public string Path(string relative) => System.IO.Path.Combine(Root, relative.Replace('/', System.IO.Path.DirectorySeparatorChar));

    public byte[] Bytes(string relative) => File.ReadAllBytes(Path(relative));

    public string Text(string relative) => File.ReadAllText(Path(relative));

    public void Dispose() => TestDirectory.Delete(Root);

    /// <summary>The repository's <c>corpus/projects</c>, found above the test assembly.</summary>
    public static string Source()
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = System.IO.Path.Combine(dir.FullName, "corpus", "projects");
            if (File.Exists(System.IO.Path.Combine(candidate, "README.md")))
            {
                return candidate;
            }
        }

        throw new InvalidOperationException("corpus/projects not found above " + AppContext.BaseDirectory);
    }

    private static void Copy(string from, string to)
    {
        Directory.CreateDirectory(to);
        foreach (var file in Directory.GetFiles(from))
        {
            File.Copy(file, System.IO.Path.Combine(to, System.IO.Path.GetFileName(file)));
        }

        foreach (var dir in Directory.GetDirectories(from))
        {
            var name = System.IO.Path.GetFileName(dir);
            if (name is "bin" or "obj")
            {
                continue;
            }

            Copy(dir, System.IO.Path.Combine(to, name));
        }
    }
}
