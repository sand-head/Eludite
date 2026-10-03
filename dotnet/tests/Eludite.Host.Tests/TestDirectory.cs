namespace Eludite.Host.Tests;

/// <summary>Cleanup for the temporary directories tests create.</summary>
internal static class TestDirectory
{
    /// <summary>
    /// Deletes <paramref name="path"/> and everything under it, unlinking links first (never following them). On
    /// Windows without symlink rights <c>ReferenceAssemblies.MergedRoot</c> makes junctions, and .NET's recursive
    /// delete removes a junction but then reports access denied.
    /// </summary>
    public static void Delete(string path)
    {
        var dir = new DirectoryInfo(path);
        if (!dir.Exists)
        {
            return;
        }

        Unlink(dir);
        dir.Delete(recursive: true);
    }

    private static void Unlink(DirectoryInfo dir)
    {
        foreach (var sub in dir.EnumerateDirectories())
        {
            if (sub.LinkTarget is not null)
            {
                sub.Delete();
            }
            else
            {
                Unlink(sub);
            }
        }
    }
}
